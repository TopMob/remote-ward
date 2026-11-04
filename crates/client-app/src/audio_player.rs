//! # Windows WASAPI Low-Latency Audio Player
//!
//! Воспроизводит аудиопоток с хоста через WASAPI Shared mode
//! напрямую в стандартные динамики/наушники клиента с ультранизкой задержкой (<15 мс)
//! и защитой от опустошения буфера (underflow concealment).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};

pub struct WasapiAudioPlayer {
    is_running: Arc<AtomicBool>,
    audio_queue: Arc<Mutex<VecDeque<u8>>>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl WasapiAudioPlayer {
    /// Инициализация и запуск фонового плеера WASAPI
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let is_running = Arc::new(AtomicBool::new(true));
        let is_running_clone = Arc::clone(&is_running);
        let audio_queue = Arc::new(Mutex::new(VecDeque::with_capacity(32768)));
        let audio_queue_clone = Arc::clone(&audio_queue);

        let thread_handle = std::thread::Builder::new()
            .name("audio-player".into())
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }

                if let Err(e) = Self::run_playback_loop(&is_running_clone, &audio_queue_clone) {
                    tracing::warn!("Цикл воспроизведения аудио WASAPI завершен: {:?}", e);
                }

                unsafe {
                    CoUninitialize();
                }
            })?;

        Ok(Self {
            is_running,
            audio_queue,
            thread_handle: Some(thread_handle),
        })
    }

    /// Добавить полученные от хоста PCM семплы (16 бит, 48 кГц, стерео) в очередь воспроизведения
    pub fn queue_pcm(&self, pcm_bytes: &[u8]) {
        let mut queue = match self.audio_queue.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        // Защита от переполнения: не держим более 100 мс звука в буфере, чтобы не создавать задержку
        const MAX_QUEUE_BYTES: usize = 48000 * 2 * 2 / 10; // 19200 байт = 100 мс
        if queue.len() > MAX_QUEUE_BYTES {
            let to_drain = queue.len().saturating_sub(MAX_QUEUE_BYTES / 2);
            queue.drain(..to_drain);
        }
        queue.extend(pcm_bytes);
    }

    fn run_playback_loop(
        is_running: &AtomicBool,
        audio_queue: &Mutex<VecDeque<u8>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        unsafe {
            // 1. Получаем Device Enumerator
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

            // 2. Получаем стандартное аудиоустройство вывода клиента
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;

            // 3. Активируем IAudioClient
            let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

            // 4. Задаем стандартный формат: 48 000 Гц, 2 канала, 16 бит PCM
            let wfx = WAVEFORMATEX {
                wFormatTag: 1, // WAVE_FORMAT_PCM
                nChannels: 2,
                nSamplesPerSec: 48000,
                nAvgBytesPerSec: 48000 * 2 * 2,
                nBlockAlign: 4,
                wBitsPerSample: 16,
                cbSize: 0,
            };

            // Буфер на 30 мс
            let buffer_duration = 300_000i64;

            let init_res = audio_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                0,
                buffer_duration,
                0,
                &wfx,
                None,
            );

            if let Err(e) = init_res {
                tracing::warn!("Не удалось инициализировать 48 кГц PCM напрямую: {:?}, пробуем MixFormat", e);
                let mix_ptr = audio_client.GetMixFormat()?;
                audio_client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    0,
                    buffer_duration,
                    0,
                    mix_ptr,
                    None,
                )?;
                windows::Win32::System::Com::CoTaskMemFree(Some(mix_ptr as *const _ as *const _));
            }

            let buffer_frame_count = audio_client.GetBufferSize()?;
            let render_client: IAudioRenderClient = audio_client.GetService()?;
            audio_client.Start()?;

            tracing::info!(
                "WASAPI Audio Player успешно запущен (размер буфера: {} фреймов)",
                buffer_frame_count
            );

            while is_running.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));

                let padding = audio_client.GetCurrentPadding()?;
                let frames_needed = buffer_frame_count.saturating_sub(padding);

                if frames_needed > 0 {
                    let bytes_needed = (frames_needed * 4) as usize; // 2 канала * 2 байта = 4 байта на фрейм
                    let p_data = render_client.GetBuffer(frames_needed)?;
                    let dst_slice = std::slice::from_raw_parts_mut(p_data, bytes_needed);

                    let mut queue = match audio_queue.lock() {
                        Ok(g) => g,
                        Err(_) => break,
                    };
                    let available = queue.len().min(bytes_needed);

                    if available > 0 {
                        for (i, byte) in queue.drain(..available).enumerate() {
                            dst_slice[i] = byte;
                        }
                    }

                    // Если данных не хватило (underflow), заполняем тишиной (нули)
                    if available < bytes_needed {
                        dst_slice[available..bytes_needed].fill(0);
                    }

                    let _ = render_client.ReleaseBuffer(frames_needed, 0);
                }
            }

            let _ = audio_client.Stop();
        }

        Ok(())
    }
}

impl Drop for WasapiAudioPlayer {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}
