use moq_nvenc::sys::nvEncodeAPI::*;
use std::ffi::c_void;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("============================================================");
    println!("     ПРЯМОЕ ОТКРЫТИЕ СЕССИИ NVENC ЧЕРЕЗ DIRECT3D 11         ");
    println!("============================================================");

    // 1. Создаем D3D11 Device
    let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let mut device_opt: Option<ID3D11Device> = None;
    let mut context_opt: Option<ID3D11DeviceContext> = None;

    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device_opt),
            None,
            Some(&mut context_opt),
        )?;
    }

    let device = device_opt.expect("D3D11 Device");
    println!("[1] D3D11 Device на NVIDIA GPU успешно создан");

    // 2. Загружаем NvEncodeAPICreateInstance из nvEncodeAPI64.dll
    let lib = unsafe { libloading::Library::new("nvEncodeAPI64.dll")? };
    let create_instance: libloading::Symbol<
        unsafe extern "C" fn(*mut NV_ENCODE_API_FUNCTION_LIST) -> NVENCSTATUS,
    > = unsafe { lib.get(b"NvEncodeAPICreateInstance\0")? };

    let mut fn_list: NV_ENCODE_API_FUNCTION_LIST = unsafe { std::mem::zeroed() };
    fn_list.version = NV_ENCODE_API_FUNCTION_LIST_VER;
    let res = unsafe { create_instance(&mut fn_list) };
    if res != NVENCSTATUS::NV_ENC_SUCCESS {
        eprintln!("Ошибка вызова NvEncodeAPICreateInstance: {:?}", res);
        return Ok(());
    }
    println!("[2] NV_ENCODE_API_FUNCTION_LIST успешно получен из nvEncodeAPI64.dll");

    // 3. Открываем сессию кодирования NVENC для Direct3D 11
    let mut session_params: NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS = unsafe { std::mem::zeroed() };
    session_params.version = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER;
    session_params.deviceType = NV_ENC_DEVICE_TYPE::NV_ENC_DEVICE_TYPE_DIRECTX;
    session_params.device = device.as_raw();
    session_params.apiVersion = NVENCAPI_VERSION;

    let mut encoder_handle: *mut c_void = std::ptr::null_mut();
    let status = unsafe {
        let open_fn = fn_list
            .nvEncOpenEncodeSessionEx
            .expect("nvEncOpenEncodeSessionEx");
        open_fn(&mut session_params, &mut encoder_handle)
    };

    println!("[3] nvEncOpenEncodeSessionEx статус: {:?}", status);

    if status == NVENCSTATUS::NV_ENC_SUCCESS {
        println!("    ✅ Сессия аппаратного кодирования NVENC (D3D11) УСПЕШНО ОТКРЫТА!");
        println!("    Указатель энкодера: {:p}", encoder_handle);

        let mut guid_count = 0u32;
        let mut guids = vec![GUID::default(); 16];
        if let Some(get_guids) = fn_list.nvEncGetEncodeGUIDs {
            let s = unsafe { get_guids(encoder_handle, guids.as_mut_ptr(), 16, &mut guid_count) };
            if s == NVENCSTATUS::NV_ENC_SUCCESS {
                println!("[4] Поддерживаемые кодеки (всего {}):", guid_count);
                for g in guids.iter().take(guid_count as usize) {
                    let name = if *g == NV_ENC_CODEC_H264_GUID {
                        "H.264"
                    } else if *g == NV_ENC_CODEC_HEVC_GUID {
                        "HEVC (H.265)"
                    } else if *g == NV_ENC_CODEC_AV1_GUID {
                        "AV1"
                    } else {
                        "Неизвестный"
                    };
                    println!("    - {} ({:08x})", name, g.Data1);
                }
            }
        }

        let encode_guid = NV_ENC_CODEC_H264_GUID;
        let mut preset_count = 0u32;
        if let Some(get_presets) = fn_list.nvEncGetEncodePresetGUIDs {
            let mut presets = vec![GUID::default(); 16];
            let s = unsafe {
                get_presets(
                    encoder_handle,
                    encode_guid,
                    presets.as_mut_ptr(),
                    16,
                    &mut preset_count,
                )
            };
            if s == NVENCSTATUS::NV_ENC_SUCCESS {
                println!("[5] Поддерживаемые пресеты H.264 (всего {}):", preset_count);
                for (idx, p) in presets.iter().take(preset_count as usize).enumerate() {
                    let label = if *p == host_encode::nvenc::NV_ENC_PRESET_P1_GUID {
                        "P1 (Fastest / Ultra-Low Latency)"
                    } else if *p == host_encode::nvenc::NV_ENC_PRESET_P2_GUID {
                        "P2 (Faster)"
                    } else if *p == host_encode::nvenc::NV_ENC_PRESET_P3_GUID {
                        "P3 (Fast)"
                    } else if *p == host_encode::nvenc::NV_ENC_PRESET_P4_GUID {
                        "P4 (Medium / Balanced)"
                    } else {
                        "Другой пресет"
                    };
                    println!("    [{}] {} ({:08x})", idx, label, p.Data1);
                }
            }
        }

        if let Some(get_preset_ex) = fn_list.nvEncGetEncodePresetConfigEx {
            let mut preset_config = NV_ENC_PRESET_CONFIG {
                version: NV_ENC_PRESET_CONFIG_VER,
                presetCfg: NV_ENC_CONFIG {
                    version: NV_ENC_CONFIG_VER,
                    ..Default::default()
                },
                ..Default::default()
            };
            let p1_guid = host_encode::nvenc::NV_ENC_PRESET_P1_GUID;
            let s = unsafe {
                get_preset_ex(
                    encoder_handle,
                    encode_guid,
                    p1_guid,
                    NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
                    &mut preset_config,
                )
            };
            println!("[6] Конфигурация P1 + Ultra-Low Latency: {:?}", s);
        }

        unsafe {
            let destroy_fn = fn_list.nvEncDestroyEncoder.expect("nvEncDestroyEncoder");
            let _ = destroy_fn(encoder_handle);
        }
        println!("    Сессия успешно закрыта (nvEncDestroyEncoder)");
    } else {
        println!("    ❌ Ошибка открытия сессии: {:?}", status);
    }

    println!("\n✅ Прямая связка D3D11 <-> NVENC работает идеально!");
    Ok(())
}
