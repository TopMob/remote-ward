use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use core_protocol::ControlMessage;

#[derive(Debug, Clone)]
pub struct CalibrationResult {
    pub min_rtt_us: u32,
    pub avg_rtt_us: u32,
    pub jitter_us: u32,
    pub packet_loss_rate: f32,
    pub connection_class: &'static str,
    pub target_bitrate_kbps: u32,
}

/// Выполнение быстрой калибровки сетевого канала (RTT, джиттер, потери, пропускная способность)
pub async fn run_network_calibration(
    socket: &UdpSocket,
    host_addr: SocketAddr,
) -> CalibrationResult {
    tracing::info!("--- Запуск калибровки сети (RTT, джиттер, пропускная способность) ---");

    let mut rtts: Vec<Duration> = Vec::with_capacity(8);
    let mut buf = [0u8; 2048];
    let ping_count = 6;
    let mut lost_pings = 0;

    // 1. Измерение RTT и джиттера пачкой пингов
    for seq in 0..ping_count {
        let now_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;

        let ping_msg = ControlMessage::Ping {
            sequence: seq as u32,
            send_timestamp_us: now_us,
        };

        if let Ok(packet) = ping_msg.to_packet() {
            let t0 = Instant::now();
            if socket.send_to(&packet, host_addr).await.is_ok() {
                let timeout_res = tokio::time::timeout(Duration::from_millis(60), async {
                    loop {
                        match socket.recv_from(&mut buf).await {
                            Ok((len, _src)) => {
                                if let Ok(ControlMessage::Pong { sequence, .. }) =
                                    ControlMessage::from_packet(&buf[..len])
                                {
                                    if sequence == seq as u32 {
                                        return Ok(t0.elapsed());
                                    }
                                }
                            }
                            Err(e) => return Err(e),
                        }
                    }
                })
                .await;

                match timeout_res {
                    Ok(Ok(rtt)) => {
                        rtts.push(rtt);
                    }
                    _ => {
                        lost_pings += 1;
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(8)).await;
    }

    // 2. Тестовый зонд пропускной способности (микропакеты данных)
    let probe_count = 6;
    let mut lost_probes = 0;
    for seq in 0..probe_count {
        let probe = ControlMessage::SpeedProbe {
            sequence: seq as u32,
            payload_size: 1200,
        };
        if let Ok(packet) = probe.to_packet() {
            let _ = socket.send_to(&packet, host_addr).await;
            let timeout_res = tokio::time::timeout(Duration::from_millis(40), async {
                loop {
                    match socket.recv_from(&mut buf).await {
                        Ok((len, _src)) => {
                            if let Ok(ControlMessage::SpeedProbeAck { sequence, .. }) =
                                ControlMessage::from_packet(&buf[..len])
                            {
                                if sequence == seq as u32 {
                                    return Ok(());
                                }
                            }
                        }
                        Err(e) => return Err(e),
                    }
                }
            })
            .await;

            if timeout_res.is_err() {
                lost_probes += 1;
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let total_sent = ping_count + probe_count;
    let total_lost = lost_pings + lost_probes;
    let packet_loss_rate = total_lost as f32 / total_sent as f32;

    if rtts.is_empty() {
        tracing::warn!("Хост не ответил на пинги калибровки (возможно брандмауэр или первый пакет). Используем безопасные настройки.");
        return CalibrationResult {
            min_rtt_us: 10_000,
            avg_rtt_us: 10_000,
            jitter_us: 1_000,
            packet_loss_rate: 0.0,
            connection_class: "Стандартная локальная сеть (Default Wi-Fi)",
            target_bitrate_kbps: 25_000,
        };
    }

    let min_rtt = *rtts.iter().min().unwrap();
    let sum_rtt: Duration = rtts.iter().sum();
    let avg_rtt = sum_rtt / rtts.len() as u32;

    // Вычисляем джиттер как среднее абсолютное отклонение от среднего RTT
    let jitter = if rtts.len() > 1 {
        let jitter_sum: f64 = rtts
            .iter()
            .map(|&r| (r.as_micros() as f64 - avg_rtt.as_micros() as f64).abs())
            .sum();
        Duration::from_micros((jitter_sum / rtts.len() as f64) as u64)
    } else {
        Duration::from_micros(0)
    };

    let avg_rtt_ms = avg_rtt.as_secs_f64() * 1000.0;
    let min_rtt_ms = min_rtt.as_secs_f64() * 1000.0;
    let jitter_ms = jitter.as_secs_f64() * 1000.0;

    let (connection_class, target_bitrate_kbps) = if avg_rtt_ms < 3.0 && packet_loss_rate < 0.02 {
        ("Прямой LAN кабель / Wi-Fi 6 (Сверхнизкая задержка)", 30_000)
    } else if avg_rtt_ms < 8.0 && packet_loss_rate < 0.05 {
        ("Качественный Wi-Fi 5GHz (Низкая задержка)", 24_000)
    } else if avg_rtt_ms < 20.0 && packet_loss_rate < 0.12 {
        ("Стандартный Wi-Fi (Средняя дальность / Стены)", 18_000)
    } else {
        ("Нестабильный Wi-Fi / Высокие помехи", 12_000)
    };

    tracing::info!("============================================================");
    tracing::info!("         РЕЗУЛЬТАТЫ КАЛИБРОВКИ СЕТИ (SPEED TEST)            ");
    tracing::info!("------------------------------------------------------------");
    tracing::info!("  Минимальный RTT:     {:.2} мс", min_rtt_ms);
    tracing::info!("  Средний RTT:         {:.2} мс", avg_rtt_ms);
    tracing::info!("  Джиттер (Jitter):    {:.2} мс", jitter_ms);
    tracing::info!("  Потери пакетов:      {:.1} %", packet_loss_rate * 100.0);
    tracing::info!("  Класс соединения:    {}", connection_class);
    tracing::info!("  Оптимальный битрейт: {} Мбит/с", target_bitrate_kbps / 1000);
    tracing::info!("============================================================");

    CalibrationResult {
        min_rtt_us: min_rtt.as_micros() as u32,
        avg_rtt_us: avg_rtt.as_micros() as u32,
        jitter_us: jitter.as_micros() as u32,
        packet_loss_rate,
        connection_class,
        target_bitrate_kbps,
    }
}
