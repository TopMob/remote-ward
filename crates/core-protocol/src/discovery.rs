//! # LAN Host Discovery Protocol
//!
//! Передача UDP-маяков (Beacons) для автоматического обнаружения хост-серверов Remote-Ward в локальной сети.

use bincode::Options;
use serde::{Deserialize, Serialize};

pub const DISCOVERY_PORT: u16 = 48002;
pub const BEACON_MAGIC: &[u8; 4] = b"RWD1";

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("Маяк обнаружения слишком короткий или имеет неверную сигнатуру")]
    InvalidMagic,
    #[error("Ошибка десериализации маяка обнаружения: {0}")]
    Deserialize(#[from] bincode::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostBeacon {
    pub server_name: String,
    pub host_ip: String,
    pub control_port: u16,
    pub video_port: u16,
    pub display_width: u32,
    pub display_height: u32,
    pub max_fps: u32,
    pub gpu_name: String,
}

impl HostBeacon {
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        let mut bytes = Vec::with_capacity(128);
        bytes.extend_from_slice(BEACON_MAGIC);
        let payload = bincode::serialize(self)?;
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DiscoveryError> {
        if bytes.len() < 4 || &bytes[..4] != BEACON_MAGIC {
            return Err(DiscoveryError::InvalidMagic);
        }
        let beacon: Self = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .allow_trailing_bytes()
            .with_limit(2048)
            .deserialize(&bytes[4..])?;
        Ok(beacon)
    }
}
