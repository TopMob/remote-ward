//! # Windows WASAPI System Audio Loopback Capture
//!
//! Захватывает весь системный звук хоста (игры, браузер, системные звуки)
//! напрямую из аудиовыхода Windows через WASAPI Loopback без сторонних виртуальных кабелей.
//! Преобразует аудио в компактный 16-битный PCM стерео поток (48 кГц) с ультранизкой задержкой (~5-10 мс).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

pub type AudioCallback = Box<dyn FnMut(&[u8], u64) + Send>;

pub struct WasapiAudioCapture {
    is_running: Arc<AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl WasapiAudioCapture {
    /// Запуск фонового потока захвата звука Windows
    pub fn start<F>(mut callback: F) -> Result<Self, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnMut(&[u8], u64) + Send + 'static,
    {
        let is_running = Arc::new(AtomicBool::new(true));
        let is_running_clone = Arc::clone(&is_running);

        let thread_handle = std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }

                if let Err(e) = Self::run_capture_loop(&is_running_clone, &mut callback) {
                    tracing::warn!("Цикл захвата аудио WASAPI завершен с ошибкой: {:?}", e);
                }

                unsafe {
                    CoUninitialize();
                }
            })?;

        Ok(Self {
            is_running,
            thread_handle: Some(thread_handle),
        })
    }

    fn run_capture_loop<F>(
        is_running: &AtomicBool,
        callback: &mut F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnMut(&[u8], u64),
    {
        unsafe {
            // 1. Получаем Device Enumerator для аудиоустройств
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

            // 2. Получаем стандартное устройство вывода (Default Audio Render Endpoint)
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;

            // 3. Активируем IAudioClient
            let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

            // 4. Получаем формат микшера Windows
            let pwfx_ptr = audio_client.GetMixFormat()?;
            let pwfx = &*pwfx_ptr;

            let sample_rate = pwfx.nSamplesPerSec;
            let channels = pwfx.nChannels;
            let bits_per_sample = pwfx.wBitsPerSample;

            tracing::info!(
                "WASAPI Loopback: устройство аудио подключено ({} Гц, {} каналов, {} бит)",
                sample_rate, channels, bits_per_sample
            );

            // Буфер на 20 мс (200 000 сотен наносекунд = 20 мс)
            let buffer_duration = 200_000i64;

            audio_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                buffer_duration,
                0,
                pwfx_ptr,
                None,
            )?;

            let capture_client: IAudioCaptureClient = audio_client.GetService()?;
            audio_client.Start()?;

            tracing::info!("WASAPI Loopback захват звука успешно запущен!");

            let is_float = if pwfx.wFormatTag == 1 {
                false // WAVE_FORMAT_PCM
            } else if pwfx.wFormatTag == 3 {
                true // WAVE_FORMAT_IEEE_FLOAT
            } else if pwfx.wFormatTag == 0xFFFE {
                // WAVE_FORMAT_EXTENSIBLE: data1 == 3 для IEEE_FLOAT
                let ext = &*(pwfx_ptr as *const WAVEFORMATEXTENSIBLE);
                ext.SubFormat.data1 == 3
            } else {
                false
            };

            CoTaskMemFree(Some(pwfx_ptr as *const _ as *const _));

            let mut pcm_i16_buf: Vec<i16> = Vec::with_capacity(4096);
            let mut byte_out_buf: Vec<u8> = Vec::with_capacity(8192);

            while is_running.load(Ordering::Relaxed) {
                // Опрашиваем аудио каждые 5 мс для минимальной задержки
                std::thread::sleep(Duration::from_millis(5));

                let mut packet_size = capture_client.GetNextPacketSize()?;
                while packet_size > 0 {
                    let mut data_ptr = std::ptr::null_mut();
                    let mut frames_available = 0u32;
                    let mut flags = 0u32;

                    let hr = capture_client.GetBuffer(
                        &mut data_ptr,
                        &mut frames_available,
                        &mut flags,
                        None,
                        None,
                    );

                    if hr.is_err() {
                        break;
                    }

                    if frames_available > 0 && !data_ptr.is_null() {
                        let total_samples = (frames_available * channels as u32) as usize;
                        pcm_i16_buf.clear();

                        if flags & 1 != 0 {
                            // AUDCLNT_BUFFERFLAGS_SILENT — тишина
                            pcm_i16_buf.resize(total_samples, 0);
                        } else if is_float {
                            // Конвертация IEEE Float32 -> Int16
                            let float_slice = std::slice::from_raw_parts(data_ptr as *const f32, total_samples);
                            for &s in float_slice {
                                let clamped = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                                pcm_i16_buf.push(clamped);
                            }
                        } else if bits_per_sample == 16 {
                            // Исходный Int16
                            let i16_slice = std::slice::from_raw_parts(data_ptr as *const i16, total_samples);
                            pcm_i16_buf.extend_from_slice(i16_slice);
                        }

                        if !pcm_i16_buf.is_empty() {
                            byte_out_buf.clear();
                            for &s in &pcm_i16_buf {
                                byte_out_buf.extend_from_slice(&s.to_le_bytes());
                            }

                            let timestamp_us = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_micros() as u64;

                            callback(&byte_out_buf, timestamp_us);
                        }
                    }

                    let _ = capture_client.ReleaseBuffer(frames_available);
                    packet_size = capture_client.GetNextPacketSize()?;
                }
            }

            let _ = audio_client.Stop();
        }

        Ok(())
    }
}

impl Drop for WasapiAudioCapture {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}
