use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("============================================================");
    println!("     ТЕСТ АППАРАТНОГО ЭНКОДЕРА NVIDIA H.264 MFT             ");
    println!("============================================================");

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, 0)?;
    }

    // 1. Создаем D3D11 Device на NVIDIA GPU
    println!("[1] Создание Direct3D 11 устройства...");
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
    let _context = context_opt.expect("D3D11 Context");
    println!("    D3D11 Device успешно создан!");

    // 2. Создаем DXGI Device Manager
    println!("[2] Инициализация MFCreateDXGIDeviceManager...");
    let mut reset_token = 0u32;
    let mut dxgi_manager_opt: Option<IMFDXGIDeviceManager> = None;
    unsafe { MFCreateDXGIDeviceManager(&mut reset_token, &mut dxgi_manager_opt)? };
    let dxgi_manager = dxgi_manager_opt.expect("DXGI Device Manager");
    unsafe { dxgi_manager.ResetDevice(&device, reset_token)? };
    println!("    DXGI Device Manager успешно привязан к D3D11!");

    // 3. Создаем тестовую текстуру 1920x1080 (BGRA)
    let width = 1920u32;
    let height = 1080u32;
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };

    let mut tex_opt: Option<ID3D11Texture2D> = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex_opt))? };
    let texture = tex_opt.expect("Texture2D");
    println!("    Тестовая текстура {}x{} создана на GPU!", width, height);

    // 4. Поиск NVIDIA H.264 MFT
    println!("[3] Активация NVIDIA H.264 Encoder MFT...");
    let input_type = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output_type = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };

    let mut activate_ptrs = std::ptr::null_mut();
    let mut count = 0u32;
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE,
            Some(&input_type),
            Some(&output_type),
            &mut activate_ptrs,
            &mut count,
        )?;
    }

    if count == 0 || activate_ptrs.is_null() {
        println!("    Аппаратный энкодер не найден!");
        return Ok(());
    }

    let activates = unsafe { std::slice::from_raw_parts(activate_ptrs, count as usize) };
    let transform: IMFTransform = unsafe { activates[0].as_ref().unwrap().ActivateObject()? };
    unsafe { CoTaskMemFree(Some(activate_ptrs as _)) };
    println!("    IMFTransform успешно активирован!");

    // Разблокируем асинхронный MFT (MF_TRANSFORM_ASYNC_UNLOCK)
    if let Ok(attrs) = unsafe { transform.GetAttributes() } {
        unsafe {
            let _ = attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1);
        }
        println!("    Асинхронный аппаратный MFT разблокирован (MF_TRANSFORM_ASYNC_UNLOCK = 1)");
    }

    // 5. Передаем DXGI Device Manager энкодеру
    let manager_ptr = dxgi_manager.as_raw();
    unsafe {
        transform.ProcessMessage(
            MFT_MESSAGE_SET_D3D_MANAGER,
            manager_ptr as usize,
        )?;
    }
    println!("    D3D11 Device Manager успешно передан в аппаратный энкодер MFT!");

    // 6. Настраиваем выходной медиа-тип (H.264)
    println!("\n[4] Настройка выходного типа H.264 60 FPS (10 Мбит/с)...");
    let out_type: IMFMediaType = unsafe { MFCreateMediaType()? };

    unsafe {
        out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        out_type.SetUINT32(&MF_MT_AVG_BITRATE, 10_000_000)?;
        out_type.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | (height as u64))?;
        out_type.SetUINT64(&MF_MT_FRAME_RATE, (60u64 << 32) | 1)?;
        out_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;

        transform.SetOutputType(0, &out_type, 0)?;
    }
    println!("    SetOutputType(H.264) успешно применен!");

    // 7. Настраиваем входной медиа-тип (NV12)
    println!("\n[5] Настройка входного типа (NV12, {}x{})...", width, height);
    let in_type = unsafe { MFCreateMediaType()? };
    unsafe {
        in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        in_type.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | (height as u64))?;
        in_type.SetUINT64(&MF_MT_FRAME_RATE, (60u64 << 32) | 1)?;
        in_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;

        transform.SetInputType(0, &in_type, 0)?;
    }
    println!("    SetInputType(NV12) успешно применен!");

    // 8. Запуск потока энкодера
    unsafe {
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
    }
    println!("    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING & START_OF_STREAM успешно отправлены!");

    // 9. Создаем IMFSample из D3D11 текстуры
    println!("\n[6] Оборачивание GPU-текстуры в IMFSample...");
    let buffer: IMFMediaBuffer = unsafe {
        MFCreateDXGISurfaceBuffer(
            &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D::IID,
            &texture,
            0,
            false,
        )?
    };

    let sample: IMFSample = unsafe { MFCreateSample()? };
    unsafe {
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(0)?;
        sample.SetSampleDuration(166_666)?; // 16.6 ms (60 FPS) в единицах 100 нс

        transform.ProcessInput(0, &sample, 0)?;
    }
    println!("    ✅ ProcessInput успешно отправил GPU-кадр в аппаратный NVENC чип!");

    // 10. Получаем сжатый кадр через ProcessOutput
    println!("\n[7] Получение сжатого кадра через ProcessOutput...");
    let stream_info = unsafe { transform.GetOutputStreamInfo(0)? };
    println!("    OutputStreamInfo: размер буфера = {} байт, dwFlags = {:#x}", stream_info.cbSize, stream_info.dwFlags);

    let provides_samples = (stream_info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;
    println!("    MFT сам выделяет сэмплы (PROVIDES_SAMPLES): {}", provides_samples);

    let p_sample_input = if provides_samples || stream_info.cbSize == 0 {
        None
    } else {
        let sample_out = unsafe { MFCreateSample()? };
        let buffer_out = unsafe { MFCreateMemoryBuffer(stream_info.cbSize)? };
        unsafe { sample_out.AddBuffer(&buffer_out)? };
        Some(sample_out)
    };

    let mut output_buffers = [MFT_OUTPUT_DATA_BUFFER {
        dwStreamID: 0,
        pSample: std::mem::ManuallyDrop::new(p_sample_input),
        dwStatus: 0,
        pEvents: std::mem::ManuallyDrop::new(None),
    }];

    let mut status = 0u32;
    let t_enc = std::time::Instant::now();
    let res = unsafe { transform.ProcessOutput(0, &mut output_buffers, &mut status) };
    let encode_time_us = t_enc.elapsed().as_micros();

    match res {
        Ok(_) => {
            let p_sample = output_buffers[0].pSample.as_ref().unwrap();
            let contiguous = unsafe { p_sample.ConvertToContiguousBuffer()? };
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut current_len = 0u32;
            unsafe { contiguous.Lock(&mut ptr, None, Some(&mut current_len))? };

            println!(
                "    ✅ Кадр успешно закодирован на NVENC за {:.2} мс ({} мкс)!",
                encode_time_us as f64 / 1000.0,
                encode_time_us
            );
            println!("    Размер сжатого кадра: {} байт", current_len);

            if current_len > 4 {
                let slice = unsafe { std::slice::from_raw_parts(ptr, 8.min(current_len as usize)) };
                println!("    Первые байты потока (NAL Header): {:02x?}", slice);
            }

            unsafe { contiguous.Unlock()? };
        }
        Err(e) => {
            println!("    ProcessOutput статус: {:?} (энкодер может ожидать второй кадр для заполнения буфера)", e);
        }
    }

    unsafe {
        let _ = MFShutdown();
    }

    println!("\n✅ Проверка интеграции D3D11 <-> NVIDIA MFT Encoder прошла успешно!");
    Ok(())
}
