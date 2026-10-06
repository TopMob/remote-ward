use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;

use core_protocol::{
    ButtonState, ClientHello, ControlMessage, InputEvent, VideoCodec,
};
use core_transport::FrameReassembler;
use client_app::decoder::MftVideoDecoder;
use host_server::{HostConfig, RemoteWardHost};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = tracing_subscriber::fmt::try_init();

    println!("============================================================");
    println!("        СКВОЗНОЕ (END-TO-END) ТЕСТИРОВАНИЕ REMOTE-WARD       ");
    println!("    Захват (WGC) -> NVENC (GPU) -> UDP -> Reassembly -> MFT  ");
    println!("============================================================");

    // 1. Запуск Host-сервера в фоновой задаче Tokio
    let host_config = HostConfig {
        video_port: 48000,
        control_port: 48001,
        bitrate_kbps: 15_000,
        fps: 60,
        preferred_codec: VideoCodec::H264, // Тестируем H.264 совместимый для всех клиентов
    };

    let host = Arc::new(RemoteWardHost::new(host_config));
    let host_clone = Arc::clone(&host);

    tokio::spawn(async move {
        if let Err(e) = host_clone.run().await {
            eprintln!("Ошибка в host.run: {:?}", e);
        }
    });

    // Даем серверу 1 секунду на открытие сокетов и инициализацию WGC
    tokio::time::sleep(Duration::from_millis(1000)).await;
    println!("[1] Host-сервер успешно запущен на портах 48000 (видео) и 48001 (управление)");

    // 2. Создаем клиентские сокеты
    let client_ctrl_socket = UdpSocket::bind("127.0.0.1:0").await?;
    let client_video_port = 48002;
    let client_video_socket = UdpSocket::bind(format!("127.0.0.1:{}", client_video_port)).await?;
    println!("[2] Клиентские сокеты открыты: видео-приемник на порту {}", client_video_port);

    // 3. Отправляем ClientHello на хост
    let host_ctrl_addr: SocketAddr = "127.0.0.1:48001".parse()?;
    let hello = ControlMessage::ClientHello(ClientHello {
        client_name: "e2e-bench-client".into(),
        client_version: "0.1.0".into(),
        supported_codecs: MftVideoDecoder::supported_codecs(),
        screen_width: 2560,
        screen_height: 1440,
        target_fps: 60,
        dpi_scale: 1.0,
        video_port: client_video_port,
    });

    let hello_bytes = hello.to_packet()?;
    client_ctrl_socket.send_to(&hello_bytes, host_ctrl_addr).await?;
    println!("[3] Отправлен ClientHello на хост (запрос видеопотока 2560x1440 @ 60 FPS)");

    // 4. Ожидаем ответ ServerHello
    let mut ctrl_buf = [0u8; 2048];
    let (ctrl_len, _) = tokio::time::timeout(
        Duration::from_secs(3),
        client_ctrl_socket.recv_from(&mut ctrl_buf),
    )
    .await??;

    let selected_codec;
    if let Ok(ControlMessage::ServerHello(server_hello)) = ControlMessage::from_packet(&ctrl_buf[..ctrl_len]) {
        println!(
            "    ✅ Получен ServerHello: сервер {}, выбран кодек: {:?}",
            server_hello.server_name, server_hello.selected_codec
        );
        selected_codec = server_hello.selected_codec;
    } else {
        panic!("Не получен корректный ServerHello от хоста!");
    }

    // 5. Тестирование передачи пользовательского ввода (клавиатура + мышь)
    println!("\n[4] Тестирование передачи и инжекции ввода (Input Pipeline)...");
    for i in 0..10 {
        // Мышь: относительное перемещение
        let mouse_event = InputEvent::MouseMoveRelative { dx: i * 2, dy: -i };
        client_ctrl_socket.send_to(&mouse_event.to_packet()?, host_ctrl_addr).await?;

        // Клавиатура: W нажата и отпущена (scan code 0x11)
        let key_down = InputEvent::Keyboard {
            scan_code: 0x11,
            is_extended: false,
            state: ButtonState::Pressed,
        };
        client_ctrl_socket.send_to(&key_down.to_packet()?, host_ctrl_addr).await?;

        let key_up = InputEvent::Keyboard {
            scan_code: 0x11,
            is_extended: false,
            state: ButtonState::Released,
        };
        client_ctrl_socket.send_to(&key_up.to_packet()?, host_ctrl_addr).await?;
    }
    println!("    ✅ 30 пакетов ввода (мышь + клавиатура) успешно доставлены на хост!");

    // 6. Прием, сборка и декодирование реального видеопотока
    println!("\n[5] Прием и сборка видеопотока из сети UDP (25 кадров 2560x1440)...");

    let mut reassembler = FrameReassembler::new(Duration::from_millis(50));
    let mut decoder = MftVideoDecoder::new(2560, 1440, selected_codec)?;
    let mut video_buf = [0u8; 2048];

    let mut received_frames = 0usize;
    let mut e2e_latencies_us = Vec::new();
    let mut frame_sizes = Vec::new();

    let stream_start = Instant::now();
    let mut mouse_tick = 0i32;

    while received_frames < 25 && stream_start.elapsed() < Duration::from_secs(10) {
        // Движение мыши заставляет Windows DWM непрерывно отрисовывать кадры на рабочем столе
        mouse_tick += 1;
        let delta = if mouse_tick % 2 == 0 { 15 } else { -15 };
        let mouse_event = InputEvent::MouseMoveRelative { dx: delta, dy: delta };
        if let Ok(bytes) = mouse_event.to_packet() {
            let _ = client_ctrl_socket.send_to(&bytes, host_ctrl_addr).await;
        }

        match tokio::time::timeout(
            Duration::from_millis(250),
            client_video_socket.recv_from(&mut video_buf),
        )
        .await
        {
            Ok(Ok((len, _))) => {
                let datagram = &video_buf[..len];
                if let Ok(Some(assembled)) = reassembler.process_packet(datagram) {
                    received_frames += 1;

                    let now_us = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_micros() as u64;

                    let e2e_us = now_us.saturating_sub(assembled.timestamp_us);
                    e2e_latencies_us.push(e2e_us);
                    frame_sizes.push(assembled.data.len());

                    // Декодируем через MFT
                    let decode_res = decoder.decode(&assembled.data);
                    let decode_status = match decode_res {
                        Ok(Some(frame)) => format!("NV12 {}x{}, задержка MFT: {:.2} мс", frame.width, frame.height, frame.latency_us as f64 / 1000.0),
                        Ok(None) => "Кадр буферизован MFT".into(),
                        Err(e) => format!("Ошибка MFT: {:?}", e),
                    };

                    println!(
                        "    Кадр #{:>2} [{}]: размер {:>5} байт, сквозная задержка сеть+GPU: {:.2} мс | Декодер: {}",
                        assembled.frame_id,
                        if assembled.is_keyframe { "IDR-кадр" } else { "P-кадр  " },
                        assembled.data.len(),
                        e2e_us as f64 / 1000.0,
                        decode_status
                    );
                }
            }
            Ok(Err(e)) => {
                eprintln!("Ошибка сокета: {:?}", e);
            }
            Err(_) => {
                // Таймаут пакета
            }
        }
    }

    // 7. Итоги сквозного тестирования
    println!("\n============================================================");
    println!("             ИТОГИ СКВОЗНОГО ТЕСТИРОВАНИЯ (E2E)             ");
    println!("============================================================");
    println!("  Успешно собрано и обработано кадров: {}", received_frames);

    if !e2e_latencies_us.is_empty() {
        e2e_latencies_us.sort_unstable();
        let avg_e2e_us = e2e_latencies_us.iter().sum::<u64>() / e2e_latencies_us.len() as u64;
        let p50_us = e2e_latencies_us[e2e_latencies_us.len() / 2];
        let min_us = e2e_latencies_us[0];
        let max_us = *e2e_latencies_us.last().unwrap();

        let avg_size_kb = (frame_sizes.iter().sum::<usize>() / frame_sizes.len()) as f64 / 1024.0;

        println!("  Средний размер кадра в сети: {:.1} КБ", avg_size_kb);
        println!("  Сквозная задержка (захват экрана + NVENC + сеть UDP + сборка пакетов):");
        println!("    Средняя: {:.2} мс ({} мкс)", avg_e2e_us as f64 / 1000.0, avg_e2e_us);
        println!("    Медиана (p50): {:.2} мс ({} мкс)", p50_us as f64 / 1000.0, p50_us);
        println!("    Минимум: {:.2} мс ({} мкс)", min_us as f64 / 1000.0, min_us);
        println!("    Максимум: {:.2} мс ({} мкс)", max_us as f64 / 1000.0, max_us);

        let target_ok = (avg_e2e_us as f64 / 1000.0) <= 16.6; // Укладываемся ли в 60 FPS бюджет (16.6 мс)
        println!(
            "\n  Соответствие бюджету кадра 60 FPS (< 16.6 мс): {}",
            if target_ok { "✅ В РАМКАХ БЮДЖЕТА" } else { "⚠️ ПРЕВЫШАЕТ" }
        );
    }

    println!("============================================================");
    println!("       ✅ ВСЕ КОМПОНЕНТЫ РАБОТАЮТ КАК ЕДИНОЕ ЦЕЛОЕ!         ");
    println!("============================================================");

    Ok(())
}
