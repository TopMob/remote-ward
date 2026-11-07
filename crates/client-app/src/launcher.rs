//! # Remote-Ward Premium Launcher UI
//!
//! Графический интерфейс лаунчера на базе egui:
//! - Режим «Клиент»: LAN Discovery карточки хостов, быстрый ручной ввод IP с валидацией,
//!   пресеты качества, переключатель кодеков (H.264 / HEVC), взаимная калибровка связки устройств.
//! - Режим «Хост-Сервер»: мониторинг статуса, локальный IP со значком копирования,
//!   аппаратные характеристики (NVIDIA NVENC, WGC, WASAPI), управление трансляцией.
//! - Внутриигровой HUD оверлей (Ctrl+Shift+O): статистика в реальном времени, кнопка отключения.

use crate::config::AppConfig;
use crate::discovery::{DiscoveredHost, LanDiscovery};
use egui::{Color32, RichText, Rounding, Stroke};
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherTab {
    Client,
    Host,
}

#[derive(Debug, Clone)]
pub enum LauncherAction {
    None,
    Connect(SocketAddr),
    Disconnect,
    SwitchMonitor,
    ToggleHostServer,
    Calibrate(SocketAddr),
}

#[derive(Debug, Clone)]
pub struct HostLiveInfo {
    pub is_running: bool,
    pub client_addr: Option<SocketAddr>,
    pub frames_streamed: u64,
}

pub struct LauncherUI {
    pub active_tab: LauncherTab,
    pub manual_ip_input: String,
    pub discovery: LanDiscovery,
    pub calibration_status: Option<String>,
    pub calibration_in_progress: bool,
    pub last_selected_host: Option<String>,
    pub error_message: Option<String>,
    pub host_server_info: Option<HostLiveInfo>,
    pub show_overlay: bool,
    pub connecting_addr: Option<SocketAddr>,
    pub copied_ip_at: Option<Instant>,
}

impl LauncherUI {
    pub fn new(config: &AppConfig) -> Self {
        Self {
            active_tab: if config.is_host_mode {
                LauncherTab::Host
            } else {
                LauncherTab::Client
            },
            manual_ip_input: config.last_host_ip.clone(),
            discovery: LanDiscovery::new(),
            calibration_status: None,
            calibration_in_progress: false,
            last_selected_host: None,
            error_message: None,
            host_server_info: None,
            show_overlay: false,
            connecting_addr: None,
            copied_ip_at: None,
        }
    }

    /// Настройка визуальной темы Obsidian / Neon Violet
    pub fn setup_theme(ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        visuals.override_text_color = Some(Color32::from_rgb(248, 250, 252));
        visuals.panel_fill = Color32::from_rgb(12, 10, 20); // #0c0a14
        visuals.window_fill = Color32::from_rgb(20, 18, 34); // #141222
        visuals.extreme_bg_color = Color32::from_rgb(10, 8, 16);
        visuals.faint_bg_color = Color32::from_rgb(30, 26, 48);

        // Кнопки и поля ввода по умолчанию
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(26, 23, 42);
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0f32, Color32::from_rgb(46, 41, 78));
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0f32, Color32::from_rgb(226, 232, 240));
        visuals.widgets.inactive.rounding = Rounding::same(6.0);

        visuals.widgets.hovered.bg_fill = Color32::from_rgb(42, 36, 68);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0f32, Color32::from_rgb(124, 58, 237));
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0f32, Color32::from_rgb(255, 255, 255));
        visuals.widgets.hovered.rounding = Rounding::same(6.0);

        visuals.widgets.active.bg_fill = Color32::from_rgb(124, 58, 237);
        visuals.widgets.active.bg_stroke = Stroke::new(1.0f32, Color32::from_rgb(139, 92, 246));
        visuals.widgets.active.fg_stroke = Stroke::new(1.0f32, Color32::from_rgb(255, 255, 255));
        visuals.widgets.active.rounding = Rounding::same(6.0);

        visuals.selection.bg_fill = Color32::from_rgb(124, 58, 237);
        visuals.selection.stroke = Stroke::new(1.0f32, Color32::from_rgb(167, 139, 250));

        ctx.set_visuals(visuals);
    }

    /// Отрисовка основного окна лаунчера
    pub fn draw_launcher(&mut self, ctx: &egui::Context, config: &mut AppConfig) -> LauncherAction {
        let mut action = LauncherAction::None;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(8.0);

            // 1. Верхняя панель (Header + Табы)
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("REMOTE-WARD")
                        .size(20.0)
                        .strong()
                        .color(Color32::from_rgb(255, 255, 255)),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("ULTRA LOW LATENCY")
                        .size(11.0)
                        .color(Color32::from_rgb(139, 92, 246)),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(12.0);
                    // Переключатель табов
                    let host_tab = ui.selectable_label(
                        self.active_tab == LauncherTab::Host,
                        RichText::new("🖥️  ХОСТ-СЕРВЕР").size(13.0),
                    );
                    if host_tab.clicked() {
                        self.active_tab = LauncherTab::Host;
                        config.is_host_mode = true;
                        config.save();
                    }

                    let client_tab = ui.selectable_label(
                        self.active_tab == LauncherTab::Client,
                        RichText::new("💻  КЛИЕНТ").size(13.0),
                    );
                    if client_tab.clicked() {
                        self.active_tab = LauncherTab::Client;
                        config.is_host_mode = false;
                        config.save();
                    }
                });
            });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);

            // 2. Тело активной вкладки
            match self.active_tab {
                LauncherTab::Client => {
                    action = self.draw_client_tab(ui, config);
                }
                LauncherTab::Host => {
                    action = self.draw_host_tab(ui, config);
                }
            }
        });

        action
    }

    fn draw_client_tab(&mut self, ui: &mut egui::Ui, config: &mut AppConfig) -> LauncherAction {
        let mut action = LauncherAction::None;

        // Показ активного баннера подключения
        if let Some(target) = self.connecting_addr {
            egui::Frame::none()
                .fill(Color32::from_rgb(30, 22, 54))
                .stroke(Stroke::new(1.0f32, Color32::from_rgb(139, 92, 246)))
                .rounding(Rounding::same(8.0))
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(format!(
                                "Подключение к хост-серверу {}... Инициализация видеопотока...",
                                target
                            ))
                            .size(13.0)
                            .strong()
                            .color(Color32::from_rgb(216, 180, 254)),
                        );
                    });
                });
            ui.add_space(8.0);
        }

        // Показ сообщения об ошибке
        let mut clear_error = false;
        if let Some(ref err) = self.error_message {
            egui::Frame::none()
                .fill(Color32::from_rgba_premultiplied(127, 29, 29, 60))
                .stroke(Stroke::new(1.0f32, Color32::from_rgb(239, 68, 68)))
                .rounding(Rounding::same(8.0))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⚠️").size(14.0));
                        ui.label(
                            RichText::new(err)
                                .color(Color32::from_rgb(252, 165, 165))
                                .size(12.0),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(RichText::new("✕").size(11.0)).clicked() {
                                clear_error = true;
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }
        if clear_error {
            self.error_message = None;
        }

        let avail_width = ui.available_width();
        let left_width = ((avail_width - 24.0) * 0.52).clamp(380.0, 520.0);
        let right_width = (avail_width - left_width - 24.0).max(340.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal(|ui| {
                // ==================== Левая колонка ====================
                ui.vertical(|ui| {
                    ui.set_width(left_width);

                    ui.label(
                        RichText::new("📡  ОБНАРУЖЕНИЕ ХОСТОВ В ЛОКАЛЬНОЙ СЕТИ")
                            .size(12.0)
                            .strong()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );
                    ui.add_space(6.0);

                    let discovered = self.discovery.get_active_hosts();

                    if discovered.is_empty() {
                        egui::Frame::none()
                            .fill(Color32::from_rgb(18, 16, 28))
                            .stroke(Stroke::new(1.0f32, Color32::from_rgb(38, 34, 60)))
                            .rounding(Rounding::same(8.0))
                            .inner_margin(16.0)
                            .show(ui, |ui| {
                                ui.vertical_centered(|ui| {
                                    ui.add_space(12.0);
                                    ui.spinner();
                                    ui.add_space(10.0);
                                    ui.label(
                                        RichText::new("Поиск хост-серверов в домашней сети...")
                                            .size(13.0)
                                            .color(Color32::from_rgb(203, 213, 225)),
                                    );
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new("Убедитесь, что приложение запущено на втором ПК во вкладке «Хост-сервер».")
                                            .size(11.0)
                                            .color(Color32::from_rgb(148, 163, 184)),
                                    );
                                    ui.add_space(12.0);
                                });
                            });
                    } else {
                        for host in discovered {
                            let card_res = self.draw_host_card(ui, &host);
                            if let Some(target_addr) = card_res {
                                config.record_ip(&host.beacon.host_ip);
                                action = LauncherAction::Connect(target_addr);
                            }
                            ui.add_space(8.0);
                        }
                    }

                    ui.add_space(14.0);

                    // Секция ручного ввода адреса
                    ui.label(
                        RichText::new("🌐  РУЧНОЙ ВВОД АДРЕСА (LAN / VPN / WAN)")
                            .size(12.0)
                            .strong()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );
                    ui.add_space(6.0);

                    ui.horizontal(|ui| {
                        let text_w = (ui.available_width() - 110.0).max(220.0);
                        let text_edit = ui.add_sized(
                            [text_w, 32.0],
                            egui::TextEdit::singleline(&mut self.manual_ip_input)
                                .hint_text("Например: 192.168.1.4 или IP:48001"),
                        );

                        if text_edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            if let Some(addr) = self.parse_manual_endpoint() {
                                config.record_ip(&self.manual_ip_input);
                                action = LauncherAction::Connect(addr);
                            } else {
                                self.error_message = Some("Неверный формат адреса хоста. Ожидается IP (например 192.168.1.4) или host:порт".to_string());
                            }
                        }

                        let is_connecting = self.connecting_addr.is_some();
                        let connect_btn = ui.add_enabled(
                            !is_connecting,
                            egui::Button::new(RichText::new("Вход").strong()),
                        );
                        if connect_btn.clicked() {
                            if let Some(addr) = self.parse_manual_endpoint() {
                                config.record_ip(&self.manual_ip_input);
                                action = LauncherAction::Connect(addr);
                            } else {
                                self.error_message = Some("Неверный формат адреса хоста. Ожидается IP (например 192.168.1.4) или host:порт".to_string());
                            }
                        }
                    });

                    // Валидация в реальном времени
                    let trimmed_input = self.manual_ip_input.trim();
                    if !trimmed_input.is_empty() {
                        if let Some(resolved) = self.parse_manual_endpoint() {
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(format!("✓ Валидный адрес для подключения: {}", resolved))
                                    .size(11.0)
                                    .color(Color32::from_rgb(52, 211, 153)),
                            );
                        } else {
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new("⚠️ Ожидается IPv4, IPv6 или host (по умолчанию порт 48001)")
                                    .size(11.0)
                                    .color(Color32::from_rgb(251, 191, 36)),
                            );
                        }
                    }

                    // Быстрые подсказки недавних IP
                    if !config.recent_ips.is_empty() {
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new("Недавние:")
                                    .size(11.0)
                                    .color(Color32::from_rgb(100, 116, 139)),
                            );
                            for ip in &config.recent_ips {
                                if ui.link(RichText::new(ip).size(11.0)).clicked() {
                                    self.manual_ip_input = ip.clone();
                                }
                            }
                        });
                    }
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                // ==================== Правая колонка ====================
                ui.vertical(|ui| {
                    ui.set_width(right_width);

                    ui.label(
                        RichText::new("⚙️  ПАРАМЕТРЫ ТРАНСЛЯЦИИ")
                            .size(12.0)
                            .strong()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );
                    ui.add_space(8.0);

                    // Пресеты качества
                    let previous_preset = config.quality_preset.clone();
                    egui::ComboBox::from_label("Пресет качества")
                        .selected_text(&config.quality_preset)
                        .show_ui(ui, |ui| {
                            let presets = [
                                "Ultra (45 Mbps, 60 FPS)",
                                "Competitive (30 Mbps, 120 FPS)",
                                "Balanced (25 Mbps, 60 FPS)",
                                "Efficiency (15 Mbps, 60 FPS, HEVC)",
                                "Custom",
                            ];
                            for p in presets {
                                if ui.selectable_value(&mut config.quality_preset, p.to_string(), p).clicked() {
                                    match p {
                                        "Ultra (45 Mbps, 60 FPS)" => {
                                            config.bitrate_mbps = 45;
                                            config.target_fps = 60;
                                            config.preferred_codec = core_protocol::VideoCodec::H264;
                                        }
                                        "Competitive (30 Mbps, 120 FPS)" => {
                                            config.bitrate_mbps = 30;
                                            config.target_fps = 120;
                                            config.preferred_codec = core_protocol::VideoCodec::H264;
                                        }
                                        "Balanced (25 Mbps, 60 FPS)" => {
                                            config.bitrate_mbps = 25;
                                            config.target_fps = 60;
                                            config.preferred_codec = core_protocol::VideoCodec::H264;
                                        }
                                        "Efficiency (15 Mbps, 60 FPS, HEVC)" => {
                                            config.bitrate_mbps = 15;
                                            config.target_fps = 60;
                                            config.preferred_codec = core_protocol::VideoCodec::HEVC;
                                        }
                                        _ => {}
                                    }
                                    config.save();
                                }
                            }
                        });

                    if config.quality_preset != previous_preset {
                        config.save();
                    }

                    ui.add_space(8.0);

                    // Выбор видеокодека
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Видеокодек:").color(Color32::from_rgb(203, 213, 225)));
                        let is_h264 = config.preferred_codec == core_protocol::VideoCodec::H264;
                        if ui.selectable_label(is_h264, "H.264 (NVENC)").clicked() {
                            config.preferred_codec = core_protocol::VideoCodec::H264;
                            if config.quality_preset == "Efficiency (15 Mbps, 60 FPS, HEVC)" {
                                config.quality_preset = "Custom".to_string();
                            }
                            config.save();
                        }
                        let is_hevc = config.preferred_codec == core_protocol::VideoCodec::HEVC;
                        if ui.selectable_label(is_hevc, "HEVC / H.265").clicked() {
                            config.preferred_codec = core_protocol::VideoCodec::HEVC;
                            config.quality_preset = "Custom".to_string();
                            config.save();
                        }
                    });

                    ui.add_space(8.0);

                    // Ползунок битрейта
                    ui.horizontal(|ui| {
                        ui.label("Битрейт:");
                        if ui.add(egui::Slider::new(&mut config.bitrate_mbps, 5..=100).suffix(" Мбит/с")).changed() {
                            Self::mark_custom_if_needed(config);
                            config.save();
                        }
                    });

                    ui.add_space(6.0);

                    // Селектор целевого FPS
                    ui.horizontal(|ui| {
                        ui.label("Частота кадров:");
                        let fps_options = [60, 90, 120, 144];
                        for fps in fps_options {
                            let is_active = config.target_fps == fps;
                            if ui.selectable_value(&mut config.target_fps, fps, format!("{} FPS", fps)).clicked()
                                && !is_active
                            {
                                Self::mark_custom_if_needed(config);
                                config.save();
                            }
                        }
                    });

                    ui.add_space(14.0);

                    // Блок взаимной калибровки канала
                    egui::Frame::none()
                        .fill(Color32::from_rgb(22, 18, 36))
                        .stroke(Stroke::new(1.0f32, Color32::from_rgb(60, 42, 98)))
                        .rounding(Rounding::same(8.0))
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("⚡ ВЗАИМНАЯ КАЛИБРОВКА СЕТИ")
                                        .size(11.0)
                                        .strong()
                                        .color(Color32::from_rgb(192, 132, 252)),
                                );
                            });
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("Замер RTT, джиттера, потерь UDP и расчет оптимального битрейта для связки устройств.")
                                    .size(11.0)
                                    .color(Color32::from_rgb(148, 163, 184)),
                            );
                            ui.add_space(8.0);

                            if self.calibration_in_progress {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.add_space(6.0);
                                    ui.label(
                                        RichText::new("Выполняются сетевые замеры (RTT, джиттер, зонды)...")
                                            .size(12.0)
                                            .color(Color32::from_rgb(192, 132, 252)),
                                    );
                                });
                                ui.add_space(6.0);
                            } else if let Some(ref status) = self.calibration_status {
                                ui.label(
                                    RichText::new(status)
                                        .size(11.5)
                                        .color(Color32::from_rgb(52, 211, 153)),
                                );
                                ui.add_space(6.0);
                            }

                            let calib_btn = ui.add_enabled(
                                !self.calibration_in_progress,
                                egui::Button::new(RichText::new("Запустить калибровку канала").strong()),
                            );
                            if calib_btn.clicked() {
                                if let Some(addr) = self.parse_manual_endpoint() {
                                    self.calibration_in_progress = true;
                                    self.calibration_status = Some("Запуск замера RTT, джиттера и пропускной способности...".to_string());
                                    action = LauncherAction::Calibrate(addr);
                                } else {
                                    self.error_message = Some("Для калибровки укажите корректный IP хост-сервера в поле ввода".to_string());
                                }
                            }
                        });

                    ui.add_space(16.0);

                    // Большая кнопка подключения
                    let is_connecting = self.connecting_addr.is_some();
                    let btn_text = if is_connecting {
                        "⏳  ПОДКЛЮЧЕНИЕ К ХОСТУ..."
                    } else {
                        "🚀  ПОДКЛЮЧИТЬСЯ К ХОСТУ"
                    };

                    let primary_connect = ui.add_sized(
                        [ui.available_width(), 44.0],
                        egui::Button::new(
                            RichText::new(btn_text)
                                .size(15.0)
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(if is_connecting {
                            Color32::from_rgb(76, 50, 120)
                        } else {
                            Color32::from_rgb(124, 58, 237)
                        }),
                    );

                    if primary_connect.clicked() && !is_connecting {
                        if let Some(addr) = self.parse_manual_endpoint() {
                            config.record_ip(&self.manual_ip_input);
                            action = LauncherAction::Connect(addr);
                        } else {
                            self.error_message = Some("Укажите корректный IP хоста в поле ввода (например, 192.168.1.4)".to_string());
                        }
                    }
                });
            });
        });

        action
    }

    fn mark_custom_if_needed(config: &mut AppConfig) {
        let is_preset_match = (config.quality_preset == "Ultra (45 Mbps, 60 FPS)"
            && config.bitrate_mbps == 45
            && config.target_fps == 60)
            || (config.quality_preset == "Competitive (30 Mbps, 120 FPS)"
                && config.bitrate_mbps == 30
                && config.target_fps == 120)
            || (config.quality_preset == "Balanced (25 Mbps, 60 FPS)"
                && config.bitrate_mbps == 25
                && config.target_fps == 60)
            || (config.quality_preset == "Efficiency (15 Mbps, 60 FPS, HEVC)"
                && config.bitrate_mbps == 15
                && config.target_fps == 60
                && config.preferred_codec == core_protocol::VideoCodec::HEVC);

        if !is_preset_match {
            config.quality_preset = "Custom".to_string();
        }
    }

    fn draw_host_card(&self, ui: &mut egui::Ui, host: &DiscoveredHost) -> Option<SocketAddr> {
        let mut clicked_addr = None;

        egui::Frame::none()
            .fill(Color32::from_rgb(22, 20, 36))
            .stroke(Stroke::new(1.0f32, Color32::from_rgb(46, 41, 78)))
            .rounding(Rounding::same(8.0))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Иконка и имя хоста
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("🖥️").size(18.0));
                            ui.label(
                                RichText::new(&host.beacon.server_name)
                                    .size(14.0)
                                    .strong()
                                    .color(Color32::from_rgb(248, 250, 252)),
                            );
                            ui.label(
                                RichText::new("🟢 ONLINE")
                                    .size(10.0)
                                    .color(Color32::from_rgb(52, 211, 153)),
                            );
                        });

                        ui.add_space(2.0);

                        ui.label(
                            RichText::new(format!(
                                "IP: {}  |  Экран: {}x{} @ {} FPS",
                                host.beacon.host_ip,
                                host.beacon.display_width,
                                host.beacon.display_height,
                                host.beacon.max_fps
                            ))
                            .size(11.0)
                            .color(Color32::from_rgb(148, 163, 184)),
                        );

                        ui.label(
                            RichText::new(format!("GPU: {}", host.beacon.gpu_name))
                                .size(11.0)
                                .color(Color32::from_rgb(167, 139, 250)),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn = ui.add_sized(
                            [110.0, 34.0],
                            egui::Button::new(
                                RichText::new("Стримить")
                                    .size(13.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(124, 58, 237)),
                        );
                        if btn.clicked() {
                            let target =
                                format!("{}:{}", host.beacon.host_ip, host.beacon.control_port);
                            if let Ok(addr) = target.parse::<SocketAddr>() {
                                clicked_addr = Some(addr);
                            }
                        }
                    });
                });
            });

        clicked_addr
    }

    fn draw_host_tab(&mut self, ui: &mut egui::Ui, _config: &mut AppConfig) -> LauncherAction {
        let mut action = LauncherAction::None;

        ui.vertical_centered(|ui| {
            ui.add_space(20.0);

            // Карточка управления хост-сервером
            egui::Frame::none()
                .fill(Color32::from_rgb(20, 18, 32))
                .stroke(Stroke::new(1.0f32, Color32::from_rgb(46, 41, 78)))
                .rounding(Rounding::same(10.0))
                .inner_margin(24.0)
                .show(ui, |ui| {
                    ui.set_width(580.0);

                    // 1. Статус службы
                    let is_running = self.host_server_info.as_ref().is_some_and(|i| i.is_running);
                    let client_opt = self.host_server_info.as_ref().and_then(|i| i.client_addr);
                    let frames = self
                        .host_server_info
                        .as_ref()
                        .map_or(0, |i| i.frames_streamed);

                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Статус службы:")
                                .size(14.0)
                                .color(Color32::from_rgb(148, 163, 184)),
                        );
                        if !is_running {
                            ui.label(
                                RichText::new("⚪ ОСТАНОВЛЕН")
                                    .size(14.0)
                                    .strong()
                                    .color(Color32::from_rgb(148, 163, 184)),
                            );
                        } else if let Some(client_ip) = client_opt {
                            ui.label(
                                RichText::new(format!(
                                    "🔵 СТРИМИНГ АКТИВЕН (клиент {})",
                                    client_ip
                                ))
                                .size(14.0)
                                .strong()
                                .color(Color32::from_rgb(96, 165, 250)),
                            );
                        } else {
                            ui.label(
                                RichText::new("🟢 АКТИВЕН (ОЖИДАНИЕ КЛИЕНТА В LAN)")
                                    .size(14.0)
                                    .strong()
                                    .color(Color32::from_rgb(52, 211, 153)),
                            );
                        }
                    });

                    ui.add_space(14.0);

                    // 2. Кнопка Запуска / Остановки хоста
                    if !is_running {
                        let start_btn = ui.add_sized(
                            [ui.available_width(), 46.0],
                            egui::Button::new(
                                RichText::new("▶   ЗАПУСТИТЬ ХОСТ-СЕРВЕР")
                                    .size(15.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(16, 185, 129)), // Изумрудный зеленый
                        );
                        if start_btn.clicked() {
                            action = LauncherAction::ToggleHostServer;
                        }
                    } else {
                        let stop_btn = ui.add_sized(
                            [ui.available_width(), 46.0],
                            egui::Button::new(
                                RichText::new("⏹   ОСТАНОВИТЬ ХОСТ-СЕРВЕР")
                                    .size(15.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(220, 38, 38)), // Красный
                        );
                        if stop_btn.clicked() {
                            action = LauncherAction::ToggleHostServer;
                        }
                    }

                    ui.add_space(16.0);
                    ui.separator();
                    ui.add_space(16.0);

                    // 3. Сетевой адрес хоста для ручного подключения
                    let local_ip = get_local_ip_string();
                    let formatted_endpoint = format!("{}:48001", local_ip);

                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Локальный IP для подключения:")
                                .size(13.0)
                                .color(Color32::from_rgb(148, 163, 184)),
                        );
                        ui.label(
                            RichText::new(&formatted_endpoint)
                                .size(14.0)
                                .strong()
                                .color(Color32::from_rgb(248, 250, 252)),
                        );

                        let is_recently_copied = self
                            .copied_ip_at
                            .is_some_and(|t| t.elapsed().as_secs_f32() < 2.0);

                        let copy_btn_text = if is_recently_copied {
                            "✓ Скопировано!"
                        } else {
                            "📋 Скопировать IP"
                        };

                        if ui.button(RichText::new(copy_btn_text).size(12.0)).clicked() {
                            ui.output_mut(|o| o.copied_text = formatted_endpoint.clone());
                            self.copied_ip_at = Some(Instant::now());
                        }
                    });

                    if is_running {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("Передано кадров: {}", frames))
                                    .size(12.0)
                                    .color(Color32::from_rgb(192, 132, 252)),
                            );
                            ui.add_space(12.0);
                            ui.label(
                                RichText::new("LAN Discovery: маяк активен (порт 48002)")
                                    .size(12.0)
                                    .color(Color32::from_rgb(52, 211, 153)),
                            );
                        });
                    }

                    ui.add_space(14.0);

                    // 4. Плашка оборудования и подсистем
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Кодирование:")
                                .size(11.0)
                                .color(Color32::from_rgb(100, 116, 139)),
                        );
                        ui.label(
                            RichText::new("NVIDIA NVENC Hardware (D3D11 Zero-Copy)")
                                .size(11.0)
                                .color(Color32::from_rgb(203, 213, 225)),
                        );
                    });

                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Захват экрана:")
                                .size(11.0)
                                .color(Color32::from_rgb(100, 116, 139)),
                        );
                        ui.label(
                            RichText::new("Windows Graphics Capture (WGC)")
                                .size(11.0)
                                .color(Color32::from_rgb(203, 213, 225)),
                        );
                        ui.add_space(16.0);
                        ui.label(
                            RichText::new("Аудио:")
                                .size(11.0)
                                .color(Color32::from_rgb(100, 116, 139)),
                        );
                        ui.label(
                            RichText::new("WASAPI Loopback 48kHz Stereo")
                                .size(11.0)
                                .color(Color32::from_rgb(203, 213, 225)),
                        );
                    });
                });
        });

        action
    }

    /// Отрисовка внутриигрового HUD оверлея поверх видеотрансляции
    pub fn draw_in_stream_hud(
        &mut self,
        ctx: &egui::Context,
        rtt_ms: f32,
        fps: u32,
        decode_ms: f64,
        bitrate_mbps: f32,
    ) -> LauncherAction {
        let mut action = LauncherAction::None;

        egui::Area::new(egui::Id::new("in_stream_hud"))
            .fixed_pos([20.0, 20.0])
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(Color32::from_rgba_premultiplied(14, 12, 24, 230))
                    .stroke(Stroke::new(1.0f32, Color32::from_rgb(124, 58, 237)))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("● REMOTE-WARD LIVE")
                                    .size(12.0)
                                    .strong()
                                    .color(Color32::from_rgb(52, 211, 153)),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(format!("{:.1} FPS", fps))
                                    .size(12.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                            ui.label(
                                RichText::new(format!("RTT: {:.1} мс", rtt_ms))
                                    .size(12.0)
                                    .color(Color32::from_rgb(148, 163, 184)),
                            );
                            ui.label(
                                RichText::new(format!("Декод: {:.2} мс", decode_ms))
                                    .size(12.0)
                                    .color(Color32::from_rgb(148, 163, 184)),
                            );
                            ui.label(
                                RichText::new(format!("{:.1} Мбит/с", bitrate_mbps))
                                    .size(12.0)
                                    .color(Color32::from_rgb(192, 132, 252)),
                            );

                            ui.add_space(12.0);

                            if ui
                                .button(RichText::new("Монитор (F8)").size(11.0))
                                .clicked()
                            {
                                action = LauncherAction::SwitchMonitor;
                            }

                            if ui
                                .button(RichText::new("Скрыть (Esc)").size(11.0))
                                .clicked()
                            {
                                self.show_overlay = false;
                            }

                            let disc_btn = ui.add(
                                egui::Button::new(
                                    RichText::new("Отключиться")
                                        .size(11.0)
                                        .color(Color32::WHITE),
                                )
                                .fill(Color32::from_rgb(220, 38, 38)),
                            );
                            if disc_btn.clicked() {
                                action = LauncherAction::Disconnect;
                            }
                        });
                    });
            });

        action
    }

    /// Парсинг введенного вручную адреса (поддержка IP, IP:порт, localhost)
    pub fn parse_manual_endpoint(&self) -> Option<SocketAddr> {
        let input = self.manual_ip_input.trim();
        if input.is_empty() {
            return None;
        }

        // Поддержка localhost
        let normalized = if input.eq_ignore_ascii_case("localhost") {
            "127.0.0.1:48001".to_string()
        } else if input.to_ascii_lowercase().starts_with("localhost:") {
            format!("127.0.0.1:{}", &input[10..])
        } else {
            input.to_string()
        };

        if let Ok(addr) = normalized.parse::<SocketAddr>() {
            return Some(addr);
        }

        // Если порт не указан, подставляем стандартный порт управления 48001
        let with_port = format!("{}:48001", normalized);
        if let Ok(addr) = with_port.parse::<SocketAddr>() {
            return Some(addr);
        }

        // Попытка DNS-резолвинга
        if let Ok(mut iter) = with_port.to_socket_addrs() {
            if let Some(addr) = iter.next() {
                return Some(addr);
            }
        }

        None
    }
}

fn get_local_ip_string() -> String {
    core_transport::get_local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}
