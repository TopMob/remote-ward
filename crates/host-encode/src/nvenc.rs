use core_protocol::VideoCodec;
use libloading::Library;
use moq_nvenc::sys::nvEncodeAPI::*;
use std::ffi::c_void;
use std::ptr;
use std::time::Instant;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};

/// GUID для пресета P1 (ultra-low latency / fastest) кодировщика NVIDIA NVENC
pub const NV_ENC_PRESET_P1_GUID: GUID = GUID {
    Data1: 0xfc0a_8d3e,
    Data2: 0x45f8,
    Data3: 0x4cf8,
    Data4: [0x80, 0xc7, 0x29, 0x88, 0x71, 0x59, 0x0e, 0xbf],
};

/// GUID для пресета P2 кодировщика NVIDIA NVENC
pub const NV_ENC_PRESET_P2_GUID: GUID = GUID {
    Data1: 0xf581_cfb8,
    Data2: 0x88d6,
    Data3: 0x4381,
    Data4: [0x93, 0xf0, 0xdf, 0x13, 0xf9, 0xc2, 0x7d, 0xab],
};

/// GUID для пресета P3 кодировщика NVIDIA NVENC
pub const NV_ENC_PRESET_P3_GUID: GUID = GUID {
    Data1: 0x3685_0110,
    Data2: 0x3a07,
    Data3: 0x441f,
    Data4: [0x94, 0xd5, 0x36, 0x70, 0x63, 0x1f, 0x91, 0xf6],
};

/// GUID для пресета P4 (balanced) кодировщика NVIDIA NVENC
pub const NV_ENC_PRESET_P4_GUID: GUID = GUID {
    Data1: 0x90a7_b826,
    Data2: 0xdf06,
    Data3: 0x4862,
    Data4: [0xb9, 0xd2, 0xcd, 0x6d, 0x73, 0xa0, 0x86, 0x81],
};

/// GUID для устаревшего пресета DEFAULT (NVIDIA SDK <= 9)
pub const NV_ENC_PRESET_DEFAULT_GUID: GUID = GUID {
    Data1: 0xfc0a_3770,
    Data2: 0x01a4,
    Data3: 0x480e,
    Data4: [0xa7, 0x76, 0x4f, 0x6e, 0x24, 0x01, 0x67, 0x22],
};

use crate::error::EncodeError;

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
    pub latency_us: u64,
}

pub struct NvencEncoder {
    _lib: Library,
    fn_list: NV_ENCODE_API_FUNCTION_LIST,
    encoder_ptr: *mut c_void,
    device_ptr: *mut c_void,
    width: u32,
    height: u32,
    fps: u32,
    encode_guid: GUID,
    preset_guid: GUID,
    output_bitstream: NV_ENC_OUTPUT_PTR,
    frame_count: u64,
    encode_config: NV_ENC_CONFIG,
    registered_resources: std::collections::HashMap<usize, NV_ENC_REGISTERED_PTR>,
}

unsafe impl Send for NvencEncoder {}

impl Drop for NvencEncoder {
    fn drop(&mut self) {
        unsafe {
            if let Some(unreg_fn) = self.fn_list.nvEncUnregisterResource {
                for (_, handle) in self.registered_resources.drain() {
                    let _ = unreg_fn(self.encoder_ptr, handle);
                }
            }
            if !self.output_bitstream.is_null() {
                if let Some(destroy_bitstream) = self.fn_list.nvEncDestroyBitstreamBuffer {
                    let _ = destroy_bitstream(self.encoder_ptr, self.output_bitstream);
                }
            }
            if !self.encoder_ptr.is_null() {
                if let Some(destroy_encoder) = self.fn_list.nvEncDestroyEncoder {
                    let _ = destroy_encoder(self.encoder_ptr);
                }
            }
        }
    }
}

unsafe impl Sync for NvencEncoder {}

impl NvencEncoder {
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn device_ptr(&self) -> *mut c_void {
        self.device_ptr
    }

    pub fn new(
        device: &ID3D11Device,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_kbps: u32,
        codec: VideoCodec,
    ) -> Result<Self, EncodeError> {
        Self::new_from_raw_device(device.as_raw(), width, height, fps, bitrate_kbps, codec)
    }

    /// Инициализация энкодера через сырой указатель устройства Direct3D 11
    pub fn new_from_raw_device(
        device_ptr: *mut c_void,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_kbps: u32,
        codec: VideoCodec,
    ) -> Result<Self, EncodeError> {
        // 1. Загрузка DLL
        let lib = unsafe {
            Library::new("nvEncodeAPI64.dll").map_err(|_e| EncodeError::EncoderNotFound(codec))?
        };

        let create_instance: libloading::Symbol<
            unsafe extern "C" fn(*mut NV_ENCODE_API_FUNCTION_LIST) -> NVENCSTATUS,
        > = unsafe {
            lib.get(b"NvEncodeAPICreateInstance\0")
                .map_err(|_e| EncodeError::EncoderNotFound(codec))?
        };

        let mut fn_list = NV_ENCODE_API_FUNCTION_LIST {
            version: NV_ENCODE_API_FUNCTION_LIST_VER,
            ..Default::default()
        };
        let res = unsafe { create_instance(&mut fn_list) };
        if res != NVENCSTATUS::NV_ENC_SUCCESS {
            return Err(EncodeError::EncoderNotFound(codec));
        }

        // 2. Открытие сессии D3D11
        let mut session_params = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS {
            version: NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER,
            deviceType: NV_ENC_DEVICE_TYPE::NV_ENC_DEVICE_TYPE_DIRECTX,
            device: device_ptr,
            apiVersion: NVENCAPI_VERSION,
            ..Default::default()
        };

        let mut encoder_ptr: *mut c_void = ptr::null_mut();
        let open_fn = fn_list.nvEncOpenEncodeSessionEx.ok_or_else(|| {
            EncodeError::MediaTypeConfigFailed("nvEncOpenEncodeSessionEx отсутствует".into())
        })?;

        let status = unsafe { open_fn(&mut session_params, &mut encoder_ptr) };
        if status != NVENCSTATUS::NV_ENC_SUCCESS {
            return Err(EncodeError::MediaTypeConfigFailed(format!(
                "Ошибка открытия сессии NVENC: {:?}",
                status
            )));
        }

        let encode_guid = match codec {
            VideoCodec::H264 => NV_ENC_CODEC_H264_GUID,
            VideoCodec::HEVC => NV_ENC_CODEC_HEVC_GUID,
            VideoCodec::AV1 => NV_ENC_CODEC_AV1_GUID,
        };

        // 3. Многоуровневое согласование пресета кодирования (P1 -> P2 -> P4 -> Legacy Default)
        let mut encode_config = NV_ENC_CONFIG {
            version: NV_ENC_CONFIG_VER,
            ..Default::default()
        };

        let mut selected_preset = NV_ENC_PRESET_P1_GUID;
        let mut selected_tuning = NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;
        let mut preset_loaded = false;

        if let Some(get_preset_ex) = fn_list.nvEncGetEncodePresetConfigEx {
            let candidates = [
                (
                    NV_ENC_PRESET_P1_GUID,
                    NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
                ),
                (
                    NV_ENC_PRESET_P1_GUID,
                    NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_LOW_LATENCY,
                ),
                (
                    NV_ENC_PRESET_P2_GUID,
                    NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
                ),
                (
                    NV_ENC_PRESET_P4_GUID,
                    NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
                ),
                (
                    NV_ENC_PRESET_P4_GUID,
                    NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_LOW_LATENCY,
                ),
            ];

            for &(candidate_preset, candidate_tuning) in &candidates {
                let mut preset_config = NV_ENC_PRESET_CONFIG {
                    version: NV_ENC_PRESET_CONFIG_VER,
                    presetCfg: NV_ENC_CONFIG {
                        version: NV_ENC_CONFIG_VER,
                        ..Default::default()
                    },
                    ..Default::default()
                };

                let status = unsafe {
                    get_preset_ex(
                        encoder_ptr,
                        encode_guid,
                        candidate_preset,
                        candidate_tuning,
                        &mut preset_config,
                    )
                };

                if status == NVENCSTATUS::NV_ENC_SUCCESS {
                    encode_config = preset_config.presetCfg;
                    selected_preset = candidate_preset;
                    selected_tuning = candidate_tuning;
                    preset_loaded = true;
                    tracing::debug!(
                        "NVENC: успешно получен пресет {:08x} с тюнингом {:?}",
                        selected_preset.Data1,
                        selected_tuning
                    );
                    break;
                }
            }
        }

        if !preset_loaded {
            if let Some(get_preset) = fn_list.nvEncGetEncodePresetConfig {
                let legacy_candidates = [
                    NV_ENC_PRESET_P1_GUID,
                    NV_ENC_PRESET_P4_GUID,
                    NV_ENC_PRESET_DEFAULT_GUID,
                ];
                for &cand in &legacy_candidates {
                    let mut preset_config = NV_ENC_PRESET_CONFIG {
                        version: NV_ENC_PRESET_CONFIG_VER,
                        presetCfg: NV_ENC_CONFIG {
                            version: NV_ENC_CONFIG_VER,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let status =
                        unsafe { get_preset(encoder_ptr, encode_guid, cand, &mut preset_config) };
                    if status == NVENCSTATUS::NV_ENC_SUCCESS {
                        encode_config = preset_config.presetCfg;
                        selected_preset = cand;
                        selected_tuning = NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_UNDEFINED;
                        preset_loaded = true;
                        tracing::info!(
                            "NVENC: применен legacy-пресет {:08x}",
                            selected_preset.Data1
                        );
                        break;
                    }
                }
            }
        }

        if !preset_loaded {
            tracing::warn!("NVENC: не удалось загрузить пресет ни из одного API, используем базовую конфигурацию");
            selected_tuning = NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_UNDEFINED;
        }

        // Настройка сверхнизкой задержки: 0 B-фреймов, CBR, двухпроходное распределение битрейта, IDR по запросу
        encode_config.version = NV_ENC_CONFIG_VER;
        encode_config.profileGUID = NV_ENC_CODEC_PROFILE_AUTOSELECT_GUID;
        // Бесконечный GOP (0xFFFFFFFF): устраняет периодические 2-секундные всплески и пульсацию мутности/ряби
        encode_config.gopLength = 0xFFFFFFFF;
        encode_config.frameIntervalP = 1; // Только I и P кадры, никаких B-кадров!

        // Rate control: CBR с расширенным VBV буфером (3 кадра) для сохранения четкости при резкой смене пикселей
        encode_config.rcParams.rateControlMode = NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_CBR;
        let avg_bitrate = bitrate_kbps * 1000;
        encode_config.rcParams.averageBitRate = avg_bitrate;
        encode_config.rcParams.maxBitRate = (avg_bitrate as f64 * 1.5) as u32; // +50% запас для динамики
        let frame_bits = avg_bitrate / fps.max(1);
        let vbv_size = frame_bits * 3;
        encode_config.rcParams.vbvBufferSize = vbv_size;
        encode_config.rcParams.vbvInitialDelay = vbv_size;
        encode_config.rcParams.set_zeroReorderDelay(1); // Нулевая задержка переупорядочивания
        encode_config.rcParams.set_strictGOPTarget(0);
        encode_config.rcParams.lowDelayKeyFrameScale = 1;

        // Двухпроходное кодирование с четвертным разрешением (Two-Pass Quarter Resolution):
        // Анализирует сложность сцены на GPU за <0.1 мс, исключая "мыло" при резком движении камеры/смене окон
        encode_config.rcParams.multiPass = NV_ENC_MULTI_PASS::NV_ENC_TWO_PASS_QUARTER_RESOLUTION;

        // Аппаратное адаптивное квантование:
        // Spatial AQ + Temporal AQ: Temporal AQ обеспечивает временную согласованность макроблоков,
        // полностью устраняя мерцание, дрожание поверхностей и "рябь" в динамике
        encode_config.rcParams.set_enableAQ(1);
        encode_config.rcParams.set_aqStrength(1); // Деликатный Spatial AQ без зернистости
        encode_config.rcParams.set_enableTemporalAQ(1);

        // Ограничение QP: безопасный диапазон, не допускающий срыва буфера и макроблоков
        encode_config.rcParams.maxQP = NV_ENC_QP {
            qpInterP: 35,
            qpInterB: 35,
            qpIntra: 28,
        };
        encode_config.rcParams.set_enableMaxQP(1);

        encode_config.rcParams.minQP = NV_ENC_QP {
            qpInterP: 10,
            qpInterB: 10,
            qpIntra: 10,
        };
        encode_config.rcParams.set_enableMinQP(1);

        // Кодеко-специфичные настройки: повтор SPS/PPS на IDR, CABAC и цветовое пространство BT.709
        match codec {
            VideoCodec::H264 => {
                let h264 = unsafe { &mut encode_config.encodeCodecConfig.h264Config };
                h264.entropyCodingMode =
                    NV_ENC_H264_ENTROPY_CODING_MODE::NV_ENC_H264_ENTROPY_CODING_MODE_CABAC;
                h264.set_repeatSPSPPS(1);
                h264.idrPeriod = 0xFFFFFFFF;
                h264.maxNumRefFrames = 1;

                // Отключаем Intra-Refresh: он вызывал полосу смазывания в динамике
                h264.intraRefreshPeriod = 0;
                h264.intraRefreshCnt = 0;

                h264.h264VUIParameters.videoSignalTypePresentFlag = 1;
                h264.h264VUIParameters.colourDescriptionPresentFlag = 1;
                h264.h264VUIParameters.colourPrimaries =
                    NV_ENC_VUI_COLOR_PRIMARIES::NV_ENC_VUI_COLOR_PRIMARIES_BT709;
                h264.h264VUIParameters.transferCharacteristics =
                    NV_ENC_VUI_TRANSFER_CHARACTERISTIC::NV_ENC_VUI_TRANSFER_CHARACTERISTIC_BT709;
                h264.h264VUIParameters.colourMatrix =
                    NV_ENC_VUI_MATRIX_COEFFS::NV_ENC_VUI_MATRIX_COEFFS_BT709;
                h264.h264VUIParameters.videoFullRangeFlag = 0; // Limited range
            }
            VideoCodec::HEVC => {
                let hevc = unsafe { &mut encode_config.encodeCodecConfig.hevcConfig };
                hevc.set_repeatSPSPPS(1);
                hevc.idrPeriod = 0xFFFFFFFF;
                hevc.maxNumRefFramesInDPB = 1;

                // Отключаем Intra-Refresh для HEVC
                hevc.intraRefreshPeriod = 0;
                hevc.intraRefreshCnt = 0;

                hevc.hevcVUIParameters.videoSignalTypePresentFlag = 1;
                hevc.hevcVUIParameters.colourDescriptionPresentFlag = 1;
                hevc.hevcVUIParameters.colourPrimaries =
                    NV_ENC_VUI_COLOR_PRIMARIES::NV_ENC_VUI_COLOR_PRIMARIES_BT709;
                hevc.hevcVUIParameters.transferCharacteristics =
                    NV_ENC_VUI_TRANSFER_CHARACTERISTIC::NV_ENC_VUI_TRANSFER_CHARACTERISTIC_BT709;
                hevc.hevcVUIParameters.colourMatrix =
                    NV_ENC_VUI_MATRIX_COEFFS::NV_ENC_VUI_MATRIX_COEFFS_BT709;
                hevc.hevcVUIParameters.videoFullRangeFlag = 0; // Limited range
            }
            _ => {}
        }

        // 4. Инициализация параметров энкодера
        let mut init_params = NV_ENC_INITIALIZE_PARAMS {
            version: NV_ENC_INITIALIZE_PARAMS_VER,
            encodeGUID: encode_guid,
            presetGUID: selected_preset,
            encodeWidth: width,
            encodeHeight: height,
            darWidth: width,
            darHeight: height,
            frameRateNum: fps,
            frameRateDen: 1,
            enablePTD: 1,
            encodeConfig: &mut encode_config,
            tuningInfo: selected_tuning,
            ..Default::default()
        };

        let init_fn = fn_list.nvEncInitializeEncoder.ok_or_else(|| {
            EncodeError::MediaTypeConfigFailed("nvEncInitializeEncoder отсутствует".into())
        })?;

        let status = unsafe { init_fn(encoder_ptr, &mut init_params) };
        if status != NVENCSTATUS::NV_ENC_SUCCESS {
            return Err(EncodeError::MediaTypeConfigFailed(format!(
                "Ошибка инициализации энкодера: {:?}",
                status
            )));
        }

        // 5. Создание выходного буфера для битового потока
        let mut create_bitstream = NV_ENC_CREATE_BITSTREAM_BUFFER {
            version: NV_ENC_CREATE_BITSTREAM_BUFFER_VER,
            ..Default::default()
        };

        let create_bs_fn = fn_list.nvEncCreateBitstreamBuffer.ok_or_else(|| {
            EncodeError::MediaTypeConfigFailed("nvEncCreateBitstreamBuffer отсутствует".into())
        })?;

        let status = unsafe { create_bs_fn(encoder_ptr, &mut create_bitstream) };
        if status != NVENCSTATUS::NV_ENC_SUCCESS {
            return Err(EncodeError::MediaTypeConfigFailed(format!(
                "Ошибка создания битового буфера: {:?}",
                status
            )));
        }

        tracing::info!(
            "NVENC Аппаратный энкодер готов: {}x{} @ {} FPS, {} кбит/с, пресет: {:08x}, тюнинг: {:?}",
            width, height, fps, bitrate_kbps, selected_preset.Data1, selected_tuning
        );

        Ok(Self {
            _lib: lib,
            fn_list,
            encoder_ptr,
            device_ptr,
            width,
            height,
            fps,
            encode_guid,
            preset_guid: selected_preset,
            output_bitstream: create_bitstream.bitstreamBuffer,
            frame_count: 0,
            encode_config,
            registered_resources: std::collections::HashMap::new(),
        })
    }

    /// Динамическое изменение битрейта на лету без перезапуска сессии кодировщика
    pub fn reconfigure_bitrate(&mut self, new_bitrate_kbps: u32) -> Result<(), EncodeError> {
        self.reconfigure(new_bitrate_kbps, self.fps)
    }

    /// Динамическое изменение целевой частоты кадров (FPS) на лету без перезапуска сессии
    pub fn reconfigure_fps(&mut self, new_fps: u32) -> Result<(), EncodeError> {
        let current_bitrate = self.encode_config.rcParams.averageBitRate / 1000;
        self.reconfigure(current_bitrate, new_fps)
    }

    /// Динамическое одновременное изменение битрейта и частоты кадров (FPS) на лету без перезапуска сессии кодировщика
    pub fn reconfigure(&mut self, new_bitrate_kbps: u32, new_fps: u32) -> Result<(), EncodeError> {
        let reconfig_fn = self.fn_list.nvEncReconfigureEncoder.ok_or_else(|| {
            EncodeError::MediaTypeConfigFailed("nvEncReconfigureEncoder отсутствует".into())
        })?;

        self.fps = if new_fps > 0 { new_fps } else { 60 };
        let avg_bitrate = new_bitrate_kbps * 1000;
        self.encode_config.rcParams.averageBitRate = avg_bitrate;
        self.encode_config.rcParams.maxBitRate = (avg_bitrate as f64 * 1.5) as u32;
        let effective_fps = self.fps;
        let frame_bits = avg_bitrate / effective_fps.max(1);
        let vbv_size = frame_bits * 3;
        self.encode_config.rcParams.vbvBufferSize = vbv_size;
        self.encode_config.rcParams.vbvInitialDelay = vbv_size;
        self.encode_config.rcParams.lowDelayKeyFrameScale = 1;

        let mut reconfig_params = NV_ENC_RECONFIGURE_PARAMS {
            version: NV_ENC_RECONFIGURE_PARAMS_VER,
            reInitEncodeParams: NV_ENC_INITIALIZE_PARAMS {
                version: NV_ENC_INITIALIZE_PARAMS_VER,
                encodeGUID: self.encode_guid,
                presetGUID: self.preset_guid,
                encodeWidth: self.width,
                encodeHeight: self.height,
                darWidth: self.width,
                darHeight: self.height,
                frameRateNum: effective_fps,
                frameRateDen: 1,
                enablePTD: 1,
                encodeConfig: &mut self.encode_config,
                tuningInfo: NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
                ..Default::default()
            },
            ..Default::default()
        };

        let status = unsafe { reconfig_fn(self.encoder_ptr, &mut reconfig_params) };
        if status != NVENCSTATUS::NV_ENC_SUCCESS {
            return Err(EncodeError::MediaTypeConfigFailed(format!(
                "Ошибка reconfigure({} kbps, {} fps): {:?}",
                new_bitrate_kbps, effective_fps, status
            )));
        }

        tracing::info!(
            "NVENC параметры успешно перенастроены: {} кбит/с @ {} FPS",
            new_bitrate_kbps,
            effective_fps
        );
        Ok(())
    }

    /// Кодирование одного кадра из текстуры Direct3D 11
    pub fn encode_frame(
        &mut self,
        texture: &ID3D11Texture2D,
        force_keyframe: bool,
    ) -> Result<EncodedFrame, EncodeError> {
        self.encode_frame_raw(texture.as_raw(), force_keyframe)
    }

    /// Кодирование кадра через сырой указатель на ID3D11Texture2D
    pub fn encode_frame_raw(
        &mut self,
        texture_ptr: *mut c_void,
        force_keyframe: bool,
    ) -> Result<EncodedFrame, EncodeError> {
        let t0 = Instant::now();

        // 1. Проверяем кэш зарегистрированных ресурсов или регистрируем текстуру Direct3D 11 в NVENC
        let ptr_key = texture_ptr as usize;
        let registered_handle = if let Some(&handle) = self.registered_resources.get(&ptr_key) {
            handle
        } else {
            let mut register_resource = NV_ENC_REGISTER_RESOURCE {
                version: NV_ENC_REGISTER_RESOURCE_VER,
                resourceType: NV_ENC_INPUT_RESOURCE_TYPE::NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX,
                width: self.width,
                height: self.height,
                pitch: self.width * 4,
                resourceToRegister: texture_ptr,
                bufferFormat: NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB,
                ..Default::default()
            };

            let reg_fn = self.fn_list.nvEncRegisterResource.ok_or_else(|| {
                EncodeError::ProcessInputFailed("nvEncRegisterResource отсутствует".into())
            })?;

            let status = unsafe { reg_fn(self.encoder_ptr, &mut register_resource) };
            let handle = if status == NVENCSTATUS::NV_ENC_SUCCESS {
                register_resource.registeredResource
            } else {
                // Если NVENC вернул ошибку (например, текстура сменилась), сбрасываем старый кэш
                if let Some(unreg_fn) = self.fn_list.nvEncUnregisterResource {
                    for (_, h) in self.registered_resources.drain() {
                        unsafe {
                            let _ = unreg_fn(self.encoder_ptr, h);
                        }
                    }
                }
                let retry_status = unsafe { reg_fn(self.encoder_ptr, &mut register_resource) };
                if retry_status != NVENCSTATUS::NV_ENC_SUCCESS {
                    return Err(EncodeError::ProcessInputFailed(format!(
                        "Ошибка регистрации текстуры: {:?}",
                        retry_status
                    )));
                }
                register_resource.registeredResource
            };

            // Ограничиваем размер кэша дескрипторов (размер пула WGC обычно 2-4)
            if self.registered_resources.len() >= 8 {
                if let Some(unreg_fn) = self.fn_list.nvEncUnregisterResource {
                    for (_, h) in self.registered_resources.drain() {
                        unsafe {
                            let _ = unreg_fn(self.encoder_ptr, h);
                        }
                    }
                }
            }

            self.registered_resources.insert(ptr_key, handle);
            handle
        };

        // 2. Отображаем ресурс для входа энкодера
        let mut map_resource = NV_ENC_MAP_INPUT_RESOURCE {
            version: NV_ENC_MAP_INPUT_RESOURCE_VER,
            registeredResource: registered_handle,
            ..Default::default()
        };

        let map_fn = self.fn_list.nvEncMapInputResource.ok_or_else(|| {
            EncodeError::ProcessInputFailed("nvEncMapInputResource отсутствует".into())
        })?;

        let status = unsafe { map_fn(self.encoder_ptr, &mut map_resource) };
        if status != NVENCSTATUS::NV_ENC_SUCCESS {
            unsafe {
                if let Some(unreg_fn) = self.fn_list.nvEncUnregisterResource {
                    let _ = unreg_fn(self.encoder_ptr, registered_handle);
                }
            }
            return Err(EncodeError::ProcessInputFailed(format!(
                "Ошибка маппинга ресурса: {:?}",
                status
            )));
        }

        let input_buffer = map_resource.mappedResource;

        // 3. Запуск кодирования кадра
        let mut pic_params = NV_ENC_PIC_PARAMS {
            version: NV_ENC_PIC_PARAMS_VER,
            inputWidth: self.width,
            inputHeight: self.height,
            inputPitch: self.width * 4,
            inputBuffer: input_buffer,
            outputBitstream: self.output_bitstream,
            bufferFmt: NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB,
            pictureStruct: NV_ENC_PIC_STRUCT::NV_ENC_PIC_STRUCT_FRAME,
            inputTimeStamp: self.frame_count,
            encodePicFlags: if force_keyframe {
                NV_ENC_PIC_FLAGS::NV_ENC_PIC_FLAG_FORCEIDR as u32
            } else {
                0
            },
            ..Default::default()
        };

        let encode_fn = self.fn_list.nvEncEncodePicture.ok_or_else(|| {
            EncodeError::ProcessInputFailed("nvEncEncodePicture отсутствует".into())
        })?;

        let status = unsafe { encode_fn(self.encoder_ptr, &mut pic_params) };
        if status != NVENCSTATUS::NV_ENC_SUCCESS {
            // SAFETY: Очистка замапленного ресурса и разрегистрация дескриптора при ошибке
            unsafe {
                if let Some(unmap_fn) = self.fn_list.nvEncUnmapInputResource {
                    let _ = unmap_fn(self.encoder_ptr, input_buffer);
                }
                if let Some(unreg_fn) = self.fn_list.nvEncUnregisterResource {
                    let _ = unreg_fn(self.encoder_ptr, registered_handle);
                }
            }
            // Удаляем недействительный дескриптор из кэша, предотвращая Use-After-Free
            self.registered_resources.remove(&ptr_key);
            return Err(EncodeError::ProcessOutputFailed(format!(
                "Ошибка encode_picture: {:?}",
                status
            )));
        }

        // 4. Блокируем битовый поток для чтения сжатых данных
        let mut lock_bitstream = NV_ENC_LOCK_BITSTREAM {
            version: NV_ENC_LOCK_BITSTREAM_VER,
            outputBitstream: self.output_bitstream,
            ..Default::default()
        };
        lock_bitstream.set_doNotWait(0); // Ждем завершения кадра на GPU

        let lock_fn = self.fn_list.nvEncLockBitstream.ok_or_else(|| {
            EncodeError::ProcessOutputFailed("nvEncLockBitstream отсутствует".into())
        })?;

        let status = unsafe { lock_fn(self.encoder_ptr, &mut lock_bitstream) };
        if status != NVENCSTATUS::NV_ENC_SUCCESS {
            // SAFETY: Обязательно размапливаем input_buffer при сбое LockBitstream
            unsafe {
                if let Some(unmap_fn) = self.fn_list.nvEncUnmapInputResource {
                    let _ = unmap_fn(self.encoder_ptr, input_buffer);
                }
            }
            return Err(EncodeError::ProcessOutputFailed(format!(
                "Ошибка lock_bitstream: {:?}",
                status
            )));
        }

        let data = unsafe {
            std::slice::from_raw_parts(
                lock_bitstream.bitstreamBufferPtr as *const u8,
                lock_bitstream.bitstreamSizeInBytes as usize,
            )
        }
        .to_vec();

        let is_keyframe = (lock_bitstream.pictureType == NV_ENC_PIC_TYPE::NV_ENC_PIC_TYPE_IDR)
            || (lock_bitstream.pictureType == NV_ENC_PIC_TYPE::NV_ENC_PIC_TYPE_I);

        // 5. Разблокировка и очистка входных ресурсов (текстура остается в кэше зарегистрированных)
        unsafe {
            if let Some(unlock_fn) = self.fn_list.nvEncUnlockBitstream {
                let _ = unlock_fn(self.encoder_ptr, self.output_bitstream);
            }
            if let Some(unmap_fn) = self.fn_list.nvEncUnmapInputResource {
                let _ = unmap_fn(self.encoder_ptr, input_buffer);
            }
        }

        self.frame_count += 1;
        let latency_us = t0.elapsed().as_micros() as u64;

        Ok(EncodedFrame {
            data,
            is_keyframe,
            latency_us,
        })
    }
}
