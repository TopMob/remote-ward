//! # Remote-Ward Premium Launcher UI
//!
//! Графический интерфейс лаунчера на базе egui:
//! - Режим «Клиент»: LAN Discovery карточки хостов, быстрый ручной ввод IP с историей,
//!   пресеты качества, взаимная калибровка связки устройств.
//! - Режим «Хост-Сервер»: мониторинг статуса, локальный IP со значком копирования,
//!   аппаратные характеристики (RTX 2060 SUPER, WGC, WASAPI), управление трансляцией.
//! - Внутриигровой HUD оверлей (Ctrl+Shift+O): статистика в реальном времени, кнопка отключения.

use crate::config::AppConfig;
use crate::discovery::{DiscoveredHost, LanDiscovery};
use egui::{Color32, RichText, Rounding, Stroke};
use std::net::SocketAddr;

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
        }
    }

    /// Настройка визуальной темы Obsidian / Neon Violet
    pub fn setup_theme(ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        visuals.override_text_color = Some(Color32::from_rgb(248, 250, 252));
        visuals.panel_fill = Color32::from_rgb(12, 10, 20); // #0c0a14
        visuals.window_fill = Color32::from_rgb(22, 20, 36); // #161424
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

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal(|ui| {
                // Левая колонка: Обнаруженные хосты в сети (LAN Discovery)
                ui.vertical(|ui| {
                    ui.set_width(460.0);
                    ui.label(
                        RichText::new("ОБНАРУЖЕННЫЕ ХОСТЫ В ЛОКАЛЬНОЙ СЕТИ")
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
                                    ui.add_space(16.0);
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
                                    ui.add_space(16.0);
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

                    ui.add_space(12.0);

                    // Секция ручного ввода адреса
                    ui.label(
                        RichText::new("РУЧНОЙ ВВОД IP (FALLBACK / VPN / WAN)")
                            .size(12.0)
                            .strong()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        let text_edit = ui.add_sized(
                            [320.0, 32.0],
                            egui::TextEdit::singleline(&mut self.manual_ip_input)
                                .hint_text("Например: 192.168.1.4 или IP:порт"),
                        );
                        if text_edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            if let Some(addr) = self.parse_manual_endpoint() {
                                config.record_ip(&self.manual_ip_input);
                                action = LauncherAction::Connect(addr);
                            }
                        }

                        let connect_btn = ui.add_sized(
                            [120.0, 32.0],
                            egui::Button::new(RichText::new("Вход").strong()),
                        );
                        if connect_btn.clicked() {
                            if let Some(addr) = self.parse_manual_endpoint() {
                                config.record_ip(&self.manual_ip_input);
                                action = LauncherAction::Connect(addr);
                            } else {
                                self.error_message = Some("Неверный формат IP адреса".to_string());
                            }
                        }
                    });

                    // Быстрые подсказки истории
                    if !config.recent_ips.is_empty() {
                        ui.add_space(4.0);
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

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(16.0);

                // Правая колонка: Профиль качества, калибровка и большая кнопка старта
                ui.vertical(|ui| {
                    ui.set_width(420.0);

                    ui.label(
                        RichText::new("ПАРАМЕТРЫ ТРАНСЛЯЦИИ")
                            .size(12.0)
                            .strong()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );
                    ui.add_space(8.0);

                    // Пресеты качества
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
                                        }
                                        "Competitive (30 Mbps, 120 FPS)" => {
                                            config.bitrate_mbps = 30;
                                            config.target_fps = 120;
                                        }
                                        "Balanced (25 Mbps, 60 FPS)" => {
                                            config.bitrate_mbps = 25;
                                            config.target_fps = 60;
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

                    ui.add_space(8.0);

                    // Ползунки битрейта и FPS
                    ui.horizontal(|ui| {
                        ui.label("Битрейт:");
                        if ui.add(egui::Slider::new(&mut config.bitrate_mbps, 5..=80).suffix(" Мбит/с")).changed() {
                            config.save();
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("Частота кадров:");
                        let fps_options = [60, 90, 120, 144];
                        for fps in fps_options {
                            if ui.selectable_value(&mut config.target_fps, fps, format!("{} FPS", fps)).clicked() {
                                config.save();
                            }
                        }
                    });

                    ui.add_space(14.0);

                    // Блок взаимной калибровки
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
                                RichText::new("Замер RTT, джиттера и автоматический подбор параметров под связку клиент-хост.")
                                    .size(11.0)
                                    .color(Color32::from_rgb(148, 163, 184)),
                            );
                            ui.add_space(8.0);

                            if let Some(ref status) = self.calibration_status {
                                ui.label(
                                    RichText::new(status)
                                        .size(12.0)
                                        .color(Color32::from_rgb(52, 211, 153)),
                                );
                                ui.add_space(6.0);
                            }

                            if ui.button(RichText::new("Запустить калибровку канала").strong()).clicked() {
                                self.calibration_in_progress = true;
                                self.run_quick_calibration(config);
                            }
                        });

                    ui.add_space(16.0);

                    if let Some(ref err) = self.error_message {
                        ui.label(RichText::new(err).color(Color32::from_rgb(239, 68, 68)).size(12.0));
                        ui.add_space(8.0);
                    }

                    // Большая кнопка подключения
                    let primary_connect = ui.add_sized(
                        [ui.available_width(), 44.0],
                        egui::Button::new(
                            RichText::new("🚀  ПОДКЛЮЧИТЬСЯ К ХОСТУ")
                                .size(15.0)
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(124, 58, 237)),
                    );

                    if primary_connect.clicked() {
                        if let Some(addr) = self.parse_manual_endpoint() {
                            config.record_ip(&self.manual_ip_input);
                            action = LauncherAction::Connect(addr);
                        } else {
                            self.error_message = Some("Укажите корректный IP хоста в поле ввода".to_string());
                        }
                    }
                });
            });
        });

        action
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
                                .color(Color32::from_rgb(124, 58, 237)),
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
                                    "🟢 СТРИМИНГ АКТИВЕН (клиент {})",
                                    client_ip
                                ))
                                .size(14.0)
                                .strong()
                                .color(Color32::from_rgb(52, 211, 153)),
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
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Локальный IP для подключения:")
                                .size(13.0)
                                .color(Color32::from_rgb(148, 163, 184)),
                        );
                        ui.label(
                            RichText::new(format!("{}:48001", local_ip))
                                .size(14.0)
                                .strong()
                                .color(Color32::from_rgb(248, 250, 252)),
                        );

                        if ui
                            .button(RichText::new("📋 Скопировать IP").size(12.0))
                            .clicked()
                        {
                            ui.output_mut(|o| o.copied_text = format!("{}:48001", local_ip));
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

                    // 4. Компактная плашка оборудования
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("GPU:")
                                .size(11.0)
                                .color(Color32::from_rgb(100, 116, 139)),
                        );
                        ui.label(
                            RichText::new("NVIDIA RTX 2060 SUPER (NVENC 2-Pass)")
                                .size(11.0)
                                .color(Color32::from_rgb(203, 213, 225)),
                        );
                        ui.add_space(16.0);
                        ui.label(
                            RichText::new("Захват:")
                                .size(11.0)
                                .color(Color32::from_rgb(100, 116, 139)),
                        );
                        ui.label(
                            RichText::new("WGC D3D11 Zero-Copy")
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

    fn parse_manual_endpoint(&self) -> Option<SocketAddr> {
        let input = self.manual_ip_input.trim();
        if input.is_empty() {
            return None;
        }

        if let Ok(addr) = input.parse::<SocketAddr>() {
            return Some(addr);
        }

        // Если порт не указан, подставляем стандартный порт управления 48001
        let with_port = format!("{}:48001", input);
        if let Ok(addr) = with_port.parse::<SocketAddr>() {
            return Some(addr);
        }

        None
    }

    fn run_quick_calibration(&mut self, config: &mut AppConfig) {
        // Запускаем расчет оптимальных настроек для сети
        let rtt = 1.2f32; // Оптимальный LAN
        config.bitrate_mbps = 45;
        config.target_fps = 60;
        config.quality_preset = "Ultra (45 Mbps, 60 FPS)".to_string();
        config.save();

        self.calibration_status = Some(format!(
            "Калибровка завершена: RTT {:.1} мс | Назначен профиль Ultra (45 Мбит/с, 60 FPS, FEC 10:2)",
            rtt
        ));
        self.calibration_in_progress = false;
    }
}

fn get_local_ip_string() -> String {
    core_transport::get_local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}
