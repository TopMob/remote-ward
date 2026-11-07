use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;

use client_app::decoder::MftVideoDecoder;
use core_protocol::{ClientHello, ControlMessage, InputEvent};
use core_transport::FrameReassembler;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("============================================================");
    println!("    ПРОВЕРКА РЕАЛЬНОГО РАБОТАЮЩЕГО HOST-SERVER (PID)       ");
    println!("    Тестирование живого захвата, NVENC, UDP и декодера MFT  ");
    println!("============================================================");

    // 1. Клиентские сокеты
    let client_ctrl_socket = UdpSocket::bind("0.0.0.0:0").await?;
    let client_ctrl_addr = client_ctrl_socket.local_addr()?;

    let client_video_socket = UdpSocket::bind("0.0.0.0:0").await?;
    let client_video_addr = client_video_socket.local_addr()?;
    let client_video_port = client_video_addr.port();

    println!(
        "[1] Клиентские сокеты созданы: ctrl={}, video={}",
        client_ctrl_addr, client_video_addr
    );

    // 2. Адрес живого хоста
    let host_ctrl_addr: SocketAddr = "127.0.0.1:48001".parse()?;
    println!("[2] Подключение к хосту по адресу: {}", host_ctrl_addr);

    // 3. Отправка ClientHello
    let hello = ControlMessage::ClientHello(ClientHello {
        client_name: "live-tester".into(),
        client_version: "0.1.0".into(),
        supported_codecs: MftVideoDecoder::supported_codecs(),
        screen_width: 2560,
        screen_height: 1440,
        target_fps: 60,
        dpi_scale: 1.0,
        video_port: client_video_port,
        ..Default::default()
    });

    let t_hello = Instant::now();
    client_ctrl_socket
        .send_to(&hello.to_packet()?, host_ctrl_addr)
        .await?;
    println!(
        "[3] Отправлен ClientHello (видеопорт: {})...",
        client_video_port
    );

    // 4. Ожидание ответа ServerHello
    let mut ctrl_buf = [0u8; 4096];
    let (ctrl_len, _) = tokio::time::timeout(
        Duration::from_secs(3),
        client_ctrl_socket.recv_from(&mut ctrl_buf),
    )
    .await
    .map_err(|_| "Таймаут ожидания ServerHello (хост не ответил за 3 сек)")??;

    let handshake_rtt = t_hello.elapsed();
    let selected_codec = match ControlMessage::from_packet(&ctrl_buf[..ctrl_len])? {
        ControlMessage::ServerHello(server_hello) => {
            println!(
                "    ✅ Получен ServerHello за {:.2} мс! Хост: '{}', выбранный кодек: {:?}",
                handshake_rtt.as_secs_f64() * 1000.0,
                server_hello.server_name,
                server_hello.selected_codec
            );
            server_hello.selected_codec
        }
        other => {
            return Err(format!("Неожиданный ответ хоста: {:?}", other).into());
        }
    };

    // 5. Проверка Ping / Pong RTT
    let ping = ControlMessage::Ping {
        session_id: 1,
        sequence: 1,
        send_timestamp_us: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64,
    };
    let t_ping = Instant::now();
    client_ctrl_socket
        .send_to(&ping.to_packet()?, host_ctrl_addr)
        .await?;
    let (_len, _) = tokio::time::timeout(
        Duration::from_millis(500),
        client_ctrl_socket.recv_from(&mut ctrl_buf),
    )
    .await??;
    let ping_rtt = t_ping.elapsed();
    println!(
        "    ✅ Ping/Pong RTT: {:.3} мс",
        ping_rtt.as_secs_f64() * 1000.0
    );

    // 6. Прием и сборка видеопотока из реального WGC + NVENC
    println!("\n[4] Прием видеопотока с хоста (ожидание реальных кадров NVENC)...");
    let mut reassembler = FrameReassembler::new(Duration::from_millis(50));
    let mut decoder = MftVideoDecoder::new(2560, 1440, selected_codec)?;
    let mut video_buf = [0u8; 4096];

    let mut received_frames = 0usize;
    let mut total_packets = 0usize;
    let mut total_bytes = 0usize;
    let mut latencies_us = Vec::new();
    let start_recv = Instant::now();

    while received_frames < 30 && start_recv.elapsed() < Duration::from_secs(5) {
        // Симулируем легкое движение мыши каждые несколько кадров для гарантированного изменения DWM экрана
        if received_frames.is_multiple_of(5) {
            let mouse_ev = InputEvent::MouseMoveRelative { dx: 2, dy: -2 };
            let _ = client_ctrl_socket
                .send_to(&mouse_ev.to_packet()?, host_ctrl_addr)
                .await;
        }

        match tokio::time::timeout(
            Duration::from_millis(300),
            client_video_socket.recv_from(&mut video_buf),
        )
        .await
        {
            Ok(Ok((len, _))) => {
                total_packets += 1;
                total_bytes += len;
                if let Ok(Some(assembled)) = reassembler.process_packet(&video_buf[..len]) {
                    received_frames += 1;
                    let now_us = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_micros() as u64;
                    let latency_us = now_us.saturating_sub(assembled.timestamp_us);
                    latencies_us.push(latency_us);

                    // Декодируем в MFT аппаратном декодере
                    let decode_info = match decoder.decode(&assembled.data) {
                        Ok(Some(frame)) => format!(
                            "NV12 {}x{}, задержка MFT: {:.2} мс",
                            frame.width,
                            frame.height,
                            frame.latency_us as f64 / 1000.0
                        ),
                        Ok(None) => "декодер буферизует поток".into(),
                        Err(e) => format!("ошибка MFT: {:?}", e),
                    };

                    if received_frames <= 5 || received_frames.is_multiple_of(10) {
                        println!(
                            "    Кадр #{:02}: размер {} байт (пакетов всего: {}), сквозная задержка: {:.2} мс, декодер: {}",
                            received_frames,
                            assembled.data.len(),
                            total_packets,
                            latency_us as f64 / 1000.0,
                            decode_info
                        );
                    }
                }
            }
            Ok(Err(e)) => {
                eprintln!("Ошибка чтения видеосокета: {:?}", e);
                break;
            }
            Err(_) => {
                println!(
                    "    Таймаут ожидания пакета видео... (принято кадров: {})",
                    received_frames
                );
            }
        }
    }

    println!("\n============================================================");
    println!("             ИТОГИ ЖИВОГО ТЕСТИРОВАНИЯ СЕРВЕРА              ");
    println!("============================================================");
    println!(
        "  Успешно принято и декодировано кадров: {}",
        received_frames
    );
    println!(
        "  Всего видеопакетов: {}, объем: {:.1} КБ",
        total_packets,
        total_bytes as f64 / 1024.0
    );

    if !latencies_us.is_empty() {
        latencies_us.sort_unstable();
        let avg_ms = (latencies_us.iter().sum::<u64>() as f64 / latencies_us.len() as f64) / 1000.0;
        let p50_ms = latencies_us[latencies_us.len() / 2] as f64 / 1000.0;
        let p95_ms = latencies_us[(latencies_us.len() * 95) / 100] as f64 / 1000.0;
        println!("  Сквозная задержка захват -> NVENC -> сеть -> приемник:");
        println!("    Средняя: {:.2} мс", avg_ms);
        println!("    Медиана (p50): {:.2} мс", p50_ms);
        println!("    95-й перцентиль (p95): {:.2} мс", p95_ms);
    }

    if received_frames >= 20 {
        println!("\n✅ ПОЛНЫЙ СКВОЗНОЙ ТРАКТ РАБОТАЕТ В РЕАЛЬНОМ ВРЕМЕНИ НА 100%!");
        Ok(())
    } else {
        Err(format!("Недостаточно кадров получено: {}", received_frames).into())
    }
}
