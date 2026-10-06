use windows::Win32::Media::MediaFoundation::{
    MFShutdown, MFStartup, MFTEnumEx, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_ALL,
    MFT_REGISTER_TYPE_INFO, MFVideoFormat_H264, MFVideoFormat_HEVC,
    MFVideoFormat_NV12, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("============================================================");
    println!("     ОБНАРУЖЕНИЕ АППАРАТНЫХ ВИДЕОЭНКОДЕРОВ (MFT)            ");
    println!("============================================================");

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, 0)?;
    }

    // 1. Поиск H.264 энкодеров
    check_encoders("H.264", &MFVideoFormat_H264);

    // 2. Поиск HEVC (H.265) энкодеров
    check_encoders("HEVC (H.265)", &MFVideoFormat_HEVC);

    unsafe {
        let _ = MFShutdown();
    }

    Ok(())
}

fn check_encoders(name: &str, output_format: &windows::core::GUID) {
    println!("\n[+] Поиск энкодеров для {}:", name);

    let input_type = MFT_REGISTER_TYPE_INFO {
        guidMajorType: windows::Win32::Media::MediaFoundation::MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };

    let output_type = MFT_REGISTER_TYPE_INFO {
        guidMajorType: windows::Win32::Media::MediaFoundation::MFMediaType_Video,
        guidSubtype: *output_format,
    };

    let mut activate_ptrs = std::ptr::null_mut();
    let mut count = 0u32;

    let hr = unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_ALL,
            Some(&input_type),
            Some(&output_type),
            &mut activate_ptrs,
            &mut count,
        )
    };

    if let Err(e) = hr {
        println!("    Ошибка MFTEnumEx: {:?}", e);
        return;
    }

    println!("    Найдено энкодеров: {}", count);

    if count > 0 && !activate_ptrs.is_null() {
        let activates = unsafe { std::slice::from_raw_parts(activate_ptrs, count as usize) };
        for (i, act_opt) in activates.iter().enumerate() {
            if let Some(act) = act_opt {
                let mut friendly_name = [0u16; 256];
                let mut name_len = 0u32;
                let _ = unsafe {
                    act.GetString(
                        &windows::Win32::Media::MediaFoundation::MFT_FRIENDLY_NAME_Attribute,
                        &mut friendly_name,
                        Some(&mut name_len),
                    )
                };

                let friendly_str = if name_len > 0 {
                    String::from_utf16_lossy(&friendly_name[..name_len as usize])
                } else {
                    "Неизвестно".to_string()
                };

                println!("    #{}: {}", i, friendly_str);
            }
        }

        unsafe {
            CoTaskMemFree(Some(activate_ptrs as _));
        }
    }
}
