use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use core_protocol::VideoCodec;
use host_encode::NvencEncoder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("============================================================");
    println!("     БЕНЧМАРК АППАРАТНОГО ЭНКОДЕРА NVIDIA NVENC (D3D11)     ");
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
    println!("[1] Direct3D 11 устройство создано на NVIDIA GPU");

    // 2. Тестируем разрешение 2560x1440 (2K)
    let width = 2560u32;
    let height = 1440u32;
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
    println!("[2] Создана 2K-текстура: {}x{} (DXGI_FORMAT_B8G8R8A8_UNORM)", width, height);

    // 3. Инициализация NvencEncoder
    println!("[3] Инициализация NvencEncoder (H.264, 60 FPS, 15 Мбит/с)...");
    let mut encoder = NvencEncoder::new(
        &device,
        width,
        height,
        60,
        15_000,
        VideoCodec::H264,
    )?;
    println!("    Инициализация завершена успешно!");

    // 4. Прогон 100 кадров
    println!("\n[4] Запуск кодирования 100 кадров 2K на GPU...");
    let mut latencies_us = Vec::with_capacity(100);
    let mut frame_sizes = Vec::with_capacity(100);

    for i in 0..100 {
        let force_idr = i == 0;
        let encoded = encoder.encode_frame(&texture, force_idr)?;

        if i == 0 {
            println!(
                "    Первый I-кадр (IDR): {} байт, задержка: {:.2} мс",
                encoded.data.len(),
                encoded.latency_us as f64 / 1000.0
            );
        }

        latencies_us.push(encoded.latency_us);
        frame_sizes.push(encoded.data.len());
    }

    // 5. Итоги
    latencies_us.sort_unstable();
    let total_us: u64 = latencies_us.iter().sum();
    let avg_us = total_us / latencies_us.len() as u64;
    let min_us = latencies_us[0];
    let max_us = *latencies_us.last().unwrap();
    let p50_us = latencies_us[latencies_us.len() / 2];
    let p95_us = latencies_us[(latencies_us.len() * 95) / 100];
    let p99_us = latencies_us[(latencies_us.len() * 99) / 100];

    let avg_size_kb = (frame_sizes.iter().sum::<usize>() / frame_sizes.len()) as f64 / 1024.0;

    println!("\n============================================================");
    println!("             ИТОГИ БЕНЧМАРКА NVIDIA NVENC (2K 1440p)        ");
    println!("============================================================");
    println!("  Успешно закодировано: {} кадров", latencies_us.len());
    println!("  Средний размер сжатого кадра: {:.1} КБ (вместо 14.7 МБ сырых!)", avg_size_kb);
    println!("  Степень сжатия: ~{:.0}:1", (14.7 * 1024.0) / avg_size_kb);

    println!("\n  Задержка кодирования кадра на GPU (RTX 2060 SUPER):");
    println!("    Средняя: {:.2} мс ({} мкс)", avg_us as f64 / 1000.0, avg_us);
    println!("    Медиана (p50): {:.2} мс ({} мкс)", p50_us as f64 / 1000.0, p50_us);
    println!("    Минимум: {:.2} мс ({} мкс)", min_us as f64 / 1000.0, min_us);
    println!("    95-й перцентиль (p95): {:.2} мс ({} мкс)", p95_us as f64 / 1000.0, p95_us);
    println!("    99-й перцентиль (p99): {:.2} мс ({} мкс)", p99_us as f64 / 1000.0, p99_us);
    println!("    Максимум: {:.2} мс ({} мкс)", max_us as f64 / 1000.0, max_us);

    let budget_ok = (avg_us as f64 / 1000.0) <= 5.0;
    println!(
        "\n  Соответствие бюджету ТЗ (2–5 мс): {}",
        if budget_ok { "✅ В РАМКАХ БЮДЖЕТА" } else { "⚠️ ПРЕВЫШАЕТ БЮДЖЕТ" }
    );
    println!("\n[5] Тестирование кодирования HEVC (H.265, 60 FPS, 15 Мбит/с)...");
    let mut hevc_encoder = NvencEncoder::new(
        &device,
        width,
        height,
        60,
        15_000,
        VideoCodec::HEVC,
    )?;
    let mut hevc_latencies = Vec::with_capacity(50);
    for i in 0..50 {
        let encoded = hevc_encoder.encode_frame(&texture, i == 0)?;
        hevc_latencies.push(encoded.latency_us);
    }
    hevc_latencies.sort_unstable();
    let hevc_avg_us: u64 = hevc_latencies.iter().sum::<u64>() / hevc_latencies.len() as u64;
    let hevc_p50_us = hevc_latencies[hevc_latencies.len() / 2];
    println!("    ✅ HEVC (H.265) успешно: средняя задержка: {:.2} мс, p50: {:.2} мс", hevc_avg_us as f64 / 1000.0, hevc_p50_us as f64 / 1000.0);

    Ok(())
}
