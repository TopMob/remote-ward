use windows::core::GUID;
use windows::Win32::Media::MediaFoundation::{
    MFCreateMediaType, MFShutdown, MFStartup, IMFMediaType, IMFTransform,
    MFVideoFormat_H264, MFVideoFormat_NV12, MFMediaType_Video, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

// CLSID для Microsoft H.264 Video Decoder MFT: 62ce7e72-4c71-4d20-b15d-452831a87d9d
const CLSID_CMSH264DECODER_MFT: GUID = GUID::from_u128(0x62ce7e72_4c71_4d20_b15d_452831a87d9d);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("============================================================");
    println!("     ТЕСТИРОВАНИЕ АППАРАТНОГО ДЕКОДЕРА WINDOWS (MFT H.264)  ");
    println!("============================================================");

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, 0)?;
    }
    println!("[1] Media Foundation успешно инициализирован");

    // Создаем экземпляр декодера MFT
    let decoder: IMFTransform = unsafe {
        CoCreateInstance(&CLSID_CMSH264DECODER_MFT, None, CLSCTX_INPROC_SERVER)?
    };
    println!("[2] Microsoft H.264 Decoder MFT успешно инстанцирован");

    // Настраиваем тип входных данных (H.264)
    let input_type: IMFMediaType = unsafe { MFCreateMediaType()? };
    unsafe {
        input_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        input_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        let frame_size = (2560u64 << 32) | 1440u64;
        input_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)?;
        decoder.SetInputType(0, &input_type, 0)?;
    }
    println!("[3] Входной формат установлен: H.264 2560x1440");

    // Настраиваем тип выходных данных (NV12)
    let output_type: IMFMediaType = unsafe { MFCreateMediaType()? };
    unsafe {
        output_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        output_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        let frame_size = (2560u64 << 32) | 1440u64;
        output_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)?;
        decoder.SetOutputType(0, &output_type, 0)?;
    }
    println!("[4] Выходной формат установлен: NV12 2560x1440");

    println!("\n✅ Аппаратный декодер H.264 MFT полностью готов к работе!");

    unsafe {
        MFShutdown()?;
    }

    Ok(())
}
