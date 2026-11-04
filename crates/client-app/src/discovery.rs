//! # LAN Discovery Listener
//!
//! Фоновый прием маяков (UDP Beacons) от хост-серверов в локальной сети.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use core_protocol::{HostBeacon, DISCOVERY_PORT};

#[derive(Clone, Debug)]
pub struct DiscoveredHost {
    pub beacon: HostBeacon,
    pub sender_addr: SocketAddr,
    pub last_seen: Instant,
}

#[derive(Clone)]
pub struct LanDiscovery {
    hosts: Arc<Mutex<HashMap<String, DiscoveredHost>>>,
}

impl LanDiscovery {
    pub fn new() -> Self {
        let hosts = Arc::new(Mutex::new(HashMap::new()));
        let hosts_clone = Arc::clone(&hosts);

        tokio::spawn(async move {
            let socket = match socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::DGRAM,
                Some(socket2::Protocol::UDP),
            ) {
                Ok(s) => {
                    let _ = s.set_reuse_address(true);
                    let _ = s.set_broadcast(true);
                    let _ = s.set_nonblocking(true);
                    let bind_addr: SocketAddr = format!("0.0.0.0:{}", DISCOVERY_PORT).parse().unwrap();
                    if let Err(e) = s.bind(&bind_addr.into()) {
                        tracing::warn!("Не удалось забиндить сокет LAN Discovery на 0.0.0.0:{}: {:?}", DISCOVERY_PORT, e);
                        return;
                    }
                    match tokio::net::UdpSocket::from_std(s.into()) {
                        Ok(sock) => sock,
                        Err(e) => {
                            tracing::warn!("Ошибка конвертации UDP сокета в Tokio: {:?}", e);
                            return;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Ошибка создания сокета LAN Discovery: {:?}", e);
                    return;
                }
            };

            let mut buf = [0u8; 1024];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((len, src)) => {
                        if let Ok(beacon) = HostBeacon::from_bytes(&buf[..len]) {
                            let key = format!("{}:{}", beacon.host_ip, beacon.control_port);
                            let mut map = hosts_clone.lock().unwrap();
                            map.insert(key, DiscoveredHost {
                                beacon,
                                sender_addr: src,
                                last_seen: Instant::now(),
                            });
                        }
                    }
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        });

        Self { hosts }
    }

    /// Получить список всех активных хостов (активны за последние 5 секунд)
    pub fn get_active_hosts(&self) -> Vec<DiscoveredHost> {
        let mut map = self.hosts.lock().unwrap();
        let now = Instant::now();
        map.retain(|_, host| now.duration_since(host.last_seen) < Duration::from_secs(5));
        let mut list: Vec<_> = map.values().cloned().collect();
        list.sort_by(|a, b| a.beacon.server_name.cmp(&b.beacon.server_name));
        list
    }
}

