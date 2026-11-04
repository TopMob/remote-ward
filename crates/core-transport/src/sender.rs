//! # PacedSender — Отправка видеопакетов с пейсингом и защитой от переполнения
//!
//! Равномерно распределяет отправку пакетов кадра по времени (pacing),
//! предотвращая взрывные всплески трафика (micro-bursts), которые переполняют
//! буферы роутеров и сетевых карт даже в гигабитной локальной сети.
//! Также сохраняет отправленные датаграммы в `NackBuffer` для возможности повторной отправки.

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::Mutex;

use crate::nack::NackBuffer;
use crate::pacer::PacketPacer;

/// Высокоточная микросекундная пауза для пейсинга пакетов
#[inline]
pub fn accurate_sleep(duration: Duration) {
    if duration.is_zero() {
        return;
    }
    let start = Instant::now();
    // Для задержек больше 2мс спим системным сном, экономя CPU
    if duration >= Duration::from_millis(2) {
        std::thread::sleep(duration - Duration::from_millis(1));
    }
    // Оставшееся микросекундное время докручиваем спин-лупом для идеальной точности
    while start.elapsed() < duration {
        std::hint::spin_loop();
    }
}

pub struct PacedSender {
    socket: Arc<UdpSocket>,
    pacer: PacketPacer,
    nack_buffer: Arc<Mutex<NackBuffer>>,
    target_fps: u32,
}

impl PacedSender {
    pub fn new(socket: Arc<UdpSocket>, target_fps: u32) -> Self {
        Self {
            socket,
            pacer: PacketPacer::default(),
            nack_buffer: Arc::new(Mutex::new(NackBuffer::new())),
            target_fps,
        }
    }

    pub fn with_pacer(
        socket: Arc<UdpSocket>,
        pacer: PacketPacer,
        nack_buffer: Arc<Mutex<NackBuffer>>,
        target_fps: u32,
    ) -> Self {
        Self {
            socket,
            pacer,
            nack_buffer,
            target_fps,
        }
    }

    pub fn nack_buffer(&self) -> Arc<Mutex<NackBuffer>> {
        Arc::clone(&self.nack_buffer)
    }

    pub fn set_target_fps(&mut self, fps: u32) {
        self.target_fps = fps;
    }

    /// Отправить все пакеты одного кадра с пейсингом и сохранением в NACK-буфер
    pub fn send_frame_packets(
        &self,
        frame_id: u32,
        packets: &[Vec<u8>],
        target_addr: SocketAddr,
    ) -> Result<(), std::io::Error> {
        let total_packets = packets.len();
        if total_packets == 0 {
            return Ok(());
        }

        // 1. Инициализируем запись в буфере NACK
        {
            let mut nb = self.nack_buffer.lock();
            nb.begin_frame(frame_id);
            for (idx, pkt) in packets.iter().enumerate() {
                nb.buffer_packet(frame_id, idx as u16, pkt.clone());
            }
        }

        // 2. Рассчитываем задержку между пакетами
        let per_packet_delay = self.pacer.calculate_packet_delay(total_packets, self.target_fps);

        // 3. Отправляем пакеты с микро-задержкой
        for (idx, packet) in packets.iter().enumerate() {
            self.socket.send_to(packet, target_addr)?;

            // После последнего пакета кадра пауза не нужна
            if idx + 1 < total_packets && !per_packet_delay.is_zero() {
                accurate_sleep(per_packet_delay);
            }
        }

        Ok(())
    }

    /// Повторная отправка пакетов по запросу NACK клиента
    pub fn handle_nack_retransmit(
        &self,
        frame_id: u32,
        missing_indices: &[u16],
        target_addr: SocketAddr,
    ) -> usize {
        let packets = {
            let nb = self.nack_buffer.lock();
            nb.get_packets_for_nack(frame_id, missing_indices)
        };

        let count = packets.len();
        for pkt in &packets {
            let _ = self.socket.send_to(pkt, target_addr);
        }

        count
    }
}

