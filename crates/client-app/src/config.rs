//! # Persistent Application Configuration
//!
//! Сохранение и загрузка настроек лаунчера (история IP, пресеты, кодеки) в `remote_ward_config.json`.

use core_protocol::VideoCodec;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppConfig {
    pub last_host_ip: String,
    pub recent_ips: Vec<String>,
    pub quality_preset: String,
    pub target_fps: u32,
    pub bitrate_mbps: u32,
    pub preferred_codec: VideoCodec,
    pub is_host_mode: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            last_host_ip: "192.168.1.4".to_string(),
            recent_ips: vec!["192.168.1.4".to_string(), "127.0.0.1".to_string()],
            quality_preset: "Ultra (45 Mbps, 60 FPS)".to_string(),
            target_fps: 60,
            bitrate_mbps: 45,
            preferred_codec: VideoCodec::H264,
            is_host_mode: false,
        }
    }
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        PathBuf::from("remote_ward_config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str::<Self>(&content) {
                    return config;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }

    pub fn record_ip(&mut self, ip: &str) {
        let ip = ip.trim().to_string();
        if ip.is_empty() {
            return;
        }
        self.last_host_ip = ip.clone();
        self.recent_ips.retain(|x| x != &ip);
        self.recent_ips.insert(0, ip);
        if self.recent_ips.len() > 10 {
            self.recent_ips.truncate(10);
        }
        self.save();
    }
}
