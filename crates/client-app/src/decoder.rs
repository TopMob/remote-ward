use std::time::Instant;
use windows::core::{Interface, GUID, VARIANT};
use windows::Win32::Media::MediaFoundation::{
    ICodecAPI, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFShutdown, MFStartup,
    IMFMediaType, IMFSample, IMFTransform, MFT_OUTPUT_DATA_BUFFER,
    MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_NV12,
    MFMediaType_Video, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE,
    MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use core_protocol::VideoCodec;
use thiserror::Error;

const CLSID_CMSH264DECODER_MFT: GUID = GUID::from_u128(0x62ce7e72_4c71_4d20_b15d_452831a87d9d);
const CLSID_MSH265DECODER_MFT: GUID = GUID::from_u128(0x420a51a3_d601_46cf_ac41_371b7b122956);

#[derive(Error, Debug)]
pub enum DecodeError {
    #[error("Ошибка Windows API: {0}")]
    Windows(#[from] windows::core::Error),
    #[error("Кодек {0:?} не поддерживается на данном клиенте")]
    UnsupportedCodec(VideoCodec),
    #[error("Ошибка обработки входного буфера: {0}")]
    ProcessInputFailed(String),
    #[error("Ошибка обработки выходного кадра: {0}")]
    ProcessOutputFailed(String),
}

pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub latency_us: u64,
}

/// Аппаратный декодер видеокадров на базе Windows Media Foundation MFT (Intel QuickSync / Direct3D)
pub struct MftVideoDecoder {
    decoder: IMFTransform,
    width: u32,
    height: u32,
    codec: VideoCodec,
    output_stream_id: u32,
    frame_count: u64,
}

impl MftVideoDecoder {
    /// Проверка доступности системного декодера MFT для данного кодека
    pub fn is_codec_supported(codec: VideoCodec) -> bool {
        let clsid = match codec {
            VideoCodec::H264 => CLSID_CMSH264DECODER_MFT,
            VideoCodec::HEVC => CLSID_MSH265DECODER_MFT,
            VideoCodec::AV1 => return false,
        };

        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = MFStartup(MF_VERSION, 0);
            let res: Result<IMFTransform, _> = CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER);
            res.is_ok()
        })
        .join()
        .unwrap_or(false)
    }

    /// Список аппаратно поддерживаемых кодеков на текущей системе клиента
    pub fn supported_codecs() -> Vec<VideoCodec> {
        let mut list = Vec::new();
        // Приоритет HEVC (если установлен кодек)
        if Self::is_codec_supported(VideoCodec::HEVC) {
            list.push(VideoCodec::HEVC);
        }
        // H.264 доступен всегда во всех версиях Windows
        if Self::is_codec_supported(VideoCodec::H264) {
            list.push(VideoCodec::H264);
        }
        if list.is_empty() {
            list.push(VideoCodec::H264);
        }
        list
    }

    pub fn new(width: u32, height: u32, codec: VideoCodec) -> Result<Self, DecodeError> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, 0)?;
        }

        let clsid = match codec {
            VideoCodec::H264 => CLSID_CMSH264DECODER_MFT,
            VideoCodec::HEVC => CLSID_MSH265DECODER_MFT,
            VideoCodec::AV1 => return Err(DecodeError::UnsupportedCodec(codec)),
        };

        let decoder: IMFTransform = unsafe {
            CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)?
        };

        // Включаем режим минимальной задержки (Low-Latency Mode)
        const CODECAPI_AV_LOW_LATENCY_MODE: GUID =
            GUID::from_u128(0x9c27891a_ed7a_40e1_88e8_b22727a024ee);
        if let Ok(codec_api) = decoder.cast::<ICodecAPI>() {
            let var = VARIANT::from(1u32);
            match unsafe { codec_api.SetValue(&CODECAPI_AV_LOW_LATENCY_MODE, &var) } {
                Ok(_) => tracing::info!("Режим CODECAPI_AV_LOW_LATENCY_MODE успешно активирован в MFT декодере"),
                Err(e) => tracing::warn!("Не удалось включить CODECAPI_AV_LOW_LATENCY_MODE в MFT: {:?}", e),
            }
        }

        // 1. Устанавливаем тип входных данных
        let input_subtype = match codec {
            VideoCodec::H264 => MFVideoFormat_H264,
            VideoCodec::HEVC => MFVideoFormat_HEVC,
            VideoCodec::AV1 => unreachable!(),
        };

        let input_type: IMFMediaType = unsafe { MFCreateMediaType()? };
        let frame_size = ((width as u64) << 32) | (height as u64);
        unsafe {
            input_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input_type.SetGUID(&MF_MT_SUBTYPE, &input_subtype)?;
            input_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)?;
            decoder.SetInputType(0, &input_type, 0)?;
        }

        // 2. Устанавливаем тип выходных данных (NV12 аппаратный)
        let output_type: IMFMediaType = unsafe { MFCreateMediaType()? };
        unsafe {
            output_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            output_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            output_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)?;
            decoder.SetOutputType(0, &output_type, 0)?;
        }

        tracing::info!(
            "Аппаратный декодер Media Foundation MFT ({:?}) успешно настроен: {}x{}",
            codec, width, height
        );

        Ok(Self {
            decoder,
            width,
            height,
            codec,
            output_stream_id: 0,
            frame_count: 0,
        })
    }

    /// Текущий активный видеокодек
    pub fn codec(&self) -> VideoCodec {
        self.codec
    }

    /// Извлечение декодированного кадра из MFT
    fn pull_output(&mut self, t0: Instant) -> Result<Option<DecodedFrame>, DecodeError> {
        let stream_info = unsafe {
            self.decoder.GetOutputStreamInfo(self.output_stream_id)?
        };

        let min_nv12_size = self.width * self.height * 3 / 2;
        let buffer_size = stream_info.cbSize.max(min_nv12_size);

        let output_sample: IMFSample = unsafe { MFCreateSample()? };
        let output_buffer = unsafe { MFCreateMemoryBuffer(buffer_size)? };
        unsafe {
            output_sample.AddBuffer(&output_buffer)?;
        }

        let mut output_data = MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: self.output_stream_id,
            pSample: std::mem::ManuallyDrop::new(Some(output_sample)),
            dwStatus: 0,
            pEvents: std::mem::ManuallyDrop::new(None),
        };

        let mut status = 0u32;
        let output_res = unsafe {
            self.decoder.ProcessOutput(0, std::slice::from_mut(&mut output_data), &mut status)
        };

        let output_sample = std::mem::ManuallyDrop::into_inner(output_data.pSample);
        let _events = std::mem::ManuallyDrop::into_inner(output_data.pEvents);

        if let Err(e) = output_res {
            if e.code() == MF_E_TRANSFORM_STREAM_CHANGE {
                // Декодер определил формат потока (SPS/PPS). Обновляем тип вывода на NV12
                let output_type: IMFMediaType = unsafe { MFCreateMediaType()? };
                let frame_size = ((self.width as u64) << 32) | (self.height as u64);
                unsafe {
                    output_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    output_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
                    output_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)?;
                    self.decoder.SetOutputType(self.output_stream_id, &output_type, 0)?;
                }
                return self.pull_output(t0);
            }
            if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
                return Ok(None);
            }
            return Err(DecodeError::ProcessOutputFailed(format!("{:?}", e)));
        }

        let sample = match output_sample {
            Some(s) => s,
            None => return Ok(None),
        };

        let media_buffer = unsafe { sample.ConvertToContiguousBuffer()? };
        let mut buf_ptr = std::ptr::null_mut();
        let mut cur_len = 0u32;
        unsafe {
            media_buffer.Lock(&mut buf_ptr, None, Some(&mut cur_len))?;
        }

        let frame_bytes = unsafe {
            std::slice::from_raw_parts(buf_ptr as *const u8, cur_len as usize).to_vec()
        };

        unsafe {
            let _ = media_buffer.Unlock();
        }

        let latency_us = t0.elapsed().as_micros() as u64;

        Ok(Some(DecodedFrame {
            width: self.width,
            height: self.height,
            data: frame_bytes,
            latency_us,
        }))
    }

    /// Подача сжатых данных NAL и извлечение декодированного кадра (NV12)
    pub fn decode(&mut self, compressed_nal: &[u8]) -> Result<Option<DecodedFrame>, DecodeError> {
        let t0 = Instant::now();

        // 1. Создаем IMFSample с полезной нагрузкой NAL
        let sample: IMFSample = unsafe { MFCreateSample()? };
        let buffer = unsafe { MFCreateMemoryBuffer(compressed_nal.len() as u32)? };

        unsafe {
            let mut ptr = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur_len = 0u32;
            buffer.Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))?;
            std::ptr::copy_nonoverlapping(compressed_nal.as_ptr(), ptr, compressed_nal.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(compressed_nal.len() as u32)?;
            sample.AddBuffer(&buffer)?;

            // Устанавливаем метку времени (в 100-нс интервалах)
            let time_100ns = self.frame_count as i64 * (10_000_000 / 60);
            let _ = sample.SetSampleTime(time_100ns);
            let _ = sample.SetSampleDuration(10_000_000 / 60);
        }
        self.frame_count += 1;

        // 2. Отправляем входной образец в MFT декодер
        let input_res = unsafe { self.decoder.ProcessInput(0, &sample, 0) };
        if let Err(e) = input_res {
            return Err(DecodeError::ProcessInputFailed(format!("{:?}", e)));
        }

        // 3. Получаем выходной декодированный кадр
        self.pull_output(t0)
    }
}

impl Drop for MftVideoDecoder {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
        }
    }
}
