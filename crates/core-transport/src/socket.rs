/// Определение локального IPv4-адреса текущего узла через UDP-сопряжение
pub fn get_local_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

/// Низкоуровневая оптимизация UDP-сокетов Windows для стриминга сверхнизкой задержки
pub fn optimize_udp_socket(socket: &tokio::net::UdpSocket) {
    #[cfg(windows)]
    {
        let sock_ref = socket2::SockRef::from(socket);

        // 1. Увеличиваем системный буфер приема ядра Windows до 4 МБ
        // Это предотвращает переполнение буфера и потерю видеопакетов при микро-всплесках I-кадров
        const BUFFER_SIZE: usize = 4 * 1024 * 1024;
        if let Err(e) = sock_ref.set_recv_buffer_size(BUFFER_SIZE) {
            tracing::debug!("Не удалось установить SO_RCVBUF 4MB: {:?}", e);
        } else {
            tracing::trace!("Установлен SO_RCVBUF: 4 МБ");
        }

        // 2. Увеличиваем буфер отправки ядра Windows до 4 МБ
        if let Err(e) = sock_ref.set_send_buffer_size(BUFFER_SIZE) {
            tracing::debug!("Не удалось установить SO_SNDBUF 4MB: {:?}", e);
        } else {
            tracing::trace!("Установлен SO_SNDBUF: 4 МБ");
        }

        // 3. Устанавливаем QoS / DSCP 46 (Expedited Forwarding, 0x2E << 2 = 0xB8)
        // Для Wi-Fi роутеров с поддержкой WMM (802.11e) это помещает видеопакеты в аппаратную очередь AC_VI/AC_VO
        if let Err(e) = sock_ref.set_tos(0xB8) {
            tracing::debug!(
                "QoS/TOS не поддерживается или ограничен групповой политикой Windows: {:?}",
                e
            );
        } else {
            tracing::trace!("Установлен QoS DSCP 46 (Expedited Forwarding) для WMM");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_optimize_udp_socket() {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        optimize_udp_socket(&socket);
    }
}
