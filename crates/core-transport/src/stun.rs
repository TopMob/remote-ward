//! # STUN Client (RFC 5389) для определения публичного IP и обхода NAT (WAN)
//!
//! Позволяет хосту и клиенту узнать свой реальный публичный IP-адрес и внешний порт,
//! выделенный роутером/NAT (NAT Traversal), для прямого подключения через Интернет
//! из другого города без необходимости арендовать дорогостоящие VPS-серверы.
//!
//! Использует публичные STUN-серверы Google и Cloudflare по умолчанию.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;

/// Магическая кука STUN (RFC 5389)
const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
/// Тип сообщения: Binding Request
const BINDING_REQUEST: u16 = 0x0001;
/// Тип сообщения: Binding Success Response
const BINDING_RESPONSE: u16 = 0x0101;
/// Атрибут: XOR-MAPPED-ADDRESS (RFC 5389)
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
/// Атрибут: MAPPED-ADDRESS (устаревший RFC 3489)
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;

/// Список надежных публичных STUN-серверов
pub const DEFAULT_STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StunResult {
    /// Внешний публичный адрес
    pub public_addr: SocketAddr,
    /// Время отклика STUN-сервера
    pub rtt_ms: u32,
}

/// Выполняет STUN Binding Request через существующий UDP-сокет
pub async fn resolve_public_address(
    socket: &UdpSocket,
    stun_server: &str,
    timeout: Duration,
) -> Result<StunResult, Box<dyn std::error::Error + Send + Sync>> {
    let server_addr: SocketAddr = tokio::net::lookup_host(stun_server)
        .await?
        .next()
        .ok_or_else(|| std::io::Error::other("STUN server DNS lookup failed"))?;

    // 1. Формируем заголовок STUN Binding Request (20 байт)
    let mut request = [0u8; 20];
    request[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    request[2..4].copy_from_slice(&0u16.to_be_bytes()); // Message Length = 0
    request[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());

    // 12 случайных байт Transaction ID
    let mut tx_id = [0u8; 12];
    for (i, b) in tx_id.iter_mut().enumerate() {
        *b = (std::process::id() as u8).wrapping_add(i as u8).wrapping_mul(31);
    }
    request[8..20].copy_from_slice(&tx_id);

    let t0 = std::time::Instant::now();
    socket.send_to(&request, server_addr).await?;

    // 2. Ожидаем ответ STUN Binding Response
    let mut buf = [0u8; 512];
    let (len, _src) = tokio::time::timeout(timeout, socket.recv_from(&mut buf)).await??;

    if len < 20 {
        return Err(Box::new(std::io::Error::other("STUN response too short")));
    }

    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    let magic = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);

    if msg_type != BINDING_RESPONSE || magic != STUN_MAGIC_COOKIE {
        return Err(Box::new(std::io::Error::other("Invalid STUN response")));
    }

    // Проверяем совпадение Transaction ID
    if &buf[8..20] != tx_id {
        return Err(Box::new(std::io::Error::other("STUN transaction ID mismatch")));
    }

    let rtt_ms = t0.elapsed().as_millis() as u32;

    // 3. Парсим атрибуты (TLV)
    let mut offset = 20;
    while offset + 4 <= len {
        let attr_type = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
        let attr_len = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]) as usize;
        offset += 4;

        if offset + attr_len > len {
            break;
        }

        if attr_type == ATTR_XOR_MAPPED_ADDRESS && attr_len >= 8 {
            let family = buf[offset + 1];
            if family == 0x01 {
                // IPv4
                let xor_port = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]);
                let port = xor_port ^ (STUN_MAGIC_COOKIE >> 16) as u16;

                let xor_ip = u32::from_be_bytes([
                    buf[offset + 4],
                    buf[offset + 5],
                    buf[offset + 6],
                    buf[offset + 7],
                ]);
                let ip_u32 = xor_ip ^ STUN_MAGIC_COOKIE;
                let ip = IpAddr::V4(Ipv4Addr::from(ip_u32));

                return Ok(StunResult {
                    public_addr: SocketAddr::new(ip, port),
                    rtt_ms,
                });
            }
        } else if attr_type == ATTR_MAPPED_ADDRESS && attr_len >= 8 {
            let family = buf[offset + 1];
            if family == 0x01 {
                let port = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]);
                let ip = IpAddr::V4(Ipv4Addr::new(
                    buf[offset + 4],
                    buf[offset + 5],
                    buf[offset + 6],
                    buf[offset + 7],
                ));
                return Ok(StunResult {
                    public_addr: SocketAddr::new(ip, port),
                    rtt_ms,
                });
            }
        }

        // Выравнивание атрибутов STUN по границе 4 байт
        offset += (attr_len + 3) & !3;
    }

    Err(Box::new(std::io::Error::other("No mapped address found in STUN response")))
}

/// Пытается определить внешний адрес, опрашивая список серверов по очереди
pub async fn discover_public_endpoint(socket: &UdpSocket) -> Option<StunResult> {
    for &server in DEFAULT_STUN_SERVERS {
        if let Ok(res) = resolve_public_address(socket, server, Duration::from_millis(1500)).await {
            tracing::info!(
                "[WAN NAT] Внешний публичный адрес определен через {}: {} (RTT: {} мс)",
                server, res.public_addr, res.rtt_ms
            );
            return Some(res);
        }
    }
    tracing::info!("[WAN NAT] Публичный STUN-адрес не определен (активен режим локальной сети LAN)");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stun_header_building() {
        let mut request = [0u8; 20];
        request[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
        request[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());

        assert_eq!(u16::from_be_bytes([request[0], request[1]]), 0x0001);
        assert_eq!(u32::from_be_bytes([request[4], request[5], request[6], request[7]]), 0x2112_A442);
    }
}

