use std::collections::HashSet;
use std::sync::Mutex;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEINPUT,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN,
    MOUSEEVENTF_XUP, VIRTUAL_KEY,
};

const XBUTTON1: u32 = 0x0001;
const XBUTTON2: u32 = 0x0002;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
};

use core_protocol::{ButtonState, InputEvent, MouseButton};
use crate::error::InputError;

/// Высокоточный инжектор пользовательского ввода в Windows с отслеживанием зажатых клавиш
pub struct WindowsInputInjector {
    screen_width: u32,
    screen_height: u32,
    pressed_keys: Mutex<HashSet<(u16, bool)>>,
    pressed_mouse_buttons: Mutex<HashSet<MouseButton>>,
}

impl WindowsInputInjector {
    /// Создает инжектор с автоматическим определением разрешения основного экрана
    pub fn new() -> Self {
        let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) } as u32;
        let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) } as u32;

        Self {
            screen_width: if screen_width > 0 { screen_width } else { 1920 },
            screen_height: if screen_height > 0 { screen_height } else { 1080 },
            pressed_keys: Mutex::new(HashSet::new()),
            pressed_mouse_buttons: Mutex::new(HashSet::new()),
        }
    }

    /// Создает инжектор с явно заданным разрешением целевого захвата
    pub fn with_dimensions(width: u32, height: u32) -> Self {
        Self {
            screen_width: if width > 0 { width } else { 1920 },
            screen_height: if height > 0 { height } else { 1080 },
            pressed_keys: Mutex::new(HashSet::new()),
            pressed_mouse_buttons: Mutex::new(HashSet::new()),
        }
    }

    /// Инжекция одного события ввода в ОС
    pub fn inject(&self, event: &InputEvent) -> Result<(), InputError> {
        match event {
            InputEvent::MouseMoveRelative { dx, dy } => {
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: *dx,
                            dy: *dy,
                            mouseData: 0,
                            dwFlags: MOUSEEVENTF_MOVE,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                self.send(&[input])
            }

            InputEvent::MouseMoveAbsolute { x, y } => {
                // Преобразование координат пикселей в нормализованный диапазон 0..65535 для основного монитора
                let norm_x = ((*x as u64 * 65535) / self.screen_width as u64) as i32;
                let norm_y = ((*y as u64 * 65535) / self.screen_height as u64) as i32;

                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: norm_x,
                            dy: norm_y,
                            mouseData: 0,
                            dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                self.send(&[input])
            }

            InputEvent::MouseButton { button, state } => {
                {
                    let mut held = self.pressed_mouse_buttons.lock().unwrap();
                    if *state == ButtonState::Pressed {
                        held.insert(*button);
                    } else {
                        held.remove(button);
                    }
                }

                let (flags, mouse_data) = match button {
                    MouseButton::Left => (
                        if *state == ButtonState::Pressed {
                            MOUSEEVENTF_LEFTDOWN
                        } else {
                            MOUSEEVENTF_LEFTUP
                        },
                        0,
                    ),
                    MouseButton::Right => (
                        if *state == ButtonState::Pressed {
                            MOUSEEVENTF_RIGHTDOWN
                        } else {
                            MOUSEEVENTF_RIGHTUP
                        },
                        0,
                    ),
                    MouseButton::Middle => (
                        if *state == ButtonState::Pressed {
                            MOUSEEVENTF_MIDDLEDOWN
                        } else {
                            MOUSEEVENTF_MIDDLEUP
                        },
                        0,
                    ),
                    MouseButton::X1 => (
                        if *state == ButtonState::Pressed {
                            MOUSEEVENTF_XDOWN
                        } else {
                            MOUSEEVENTF_XUP
                        },
                        XBUTTON1,
                    ),
                    MouseButton::X2 => (
                        if *state == ButtonState::Pressed {
                            MOUSEEVENTF_XDOWN
                        } else {
                            MOUSEEVENTF_XUP
                        },
                        XBUTTON2,
                    ),
                };

                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0,
                            dy: 0,
                            mouseData: mouse_data,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                self.send(&[input])
            }

            InputEvent::MouseWheel { delta_y, delta_x } => {
                let mut inputs = Vec::with_capacity(2);

                if *delta_y != 0 {
                    inputs.push(INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 {
                            mi: MOUSEINPUT {
                                dx: 0,
                                dy: 0,
                                mouseData: *delta_y as u32,
                                dwFlags: MOUSEEVENTF_WHEEL,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    });
                }

                if *delta_x != 0 {
                    inputs.push(INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 {
                            mi: MOUSEINPUT {
                                dx: 0,
                                dy: 0,
                                mouseData: *delta_x as u32,
                                dwFlags: MOUSEEVENTF_HWHEEL,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    });
                }

                if !inputs.is_empty() {
                    self.send(&inputs)?;
                }
                Ok(())
            }

            InputEvent::Keyboard {
                scan_code,
                is_extended,
                state,
            } => {
                {
                    let mut held = self.pressed_keys.lock().unwrap();
                    if *state == ButtonState::Pressed {
                        held.insert((*scan_code, *is_extended));
                    } else {
                        held.remove(&(*scan_code, *is_extended));
                    }
                }

                let mut flags = KEYEVENTF_SCANCODE;
                if *is_extended {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                if *state == ButtonState::Released {
                    flags |= KEYEVENTF_KEYUP;
                }

                let input = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(0),
                            wScan: *scan_code,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                self.send(&[input])
            }

            InputEvent::Gamepad(_gamepad) => {
                // Геймпад обрабатывается через виртуальный XInput контроллер (ViGEmBus) при наличии драйвера.
                // В базовой поставке игнорируем без ошибки.
                Ok(())
            }
        }
    }

    fn send(&self, inputs: &[INPUT]) -> Result<(), InputError> {
        let sent = unsafe {
            SendInput(inputs, std::mem::size_of::<INPUT>() as i32)
        };
        if sent != inputs.len() as u32 {
            let err = unsafe { windows::Win32::Foundation::GetLastError() };
            return Err(InputError::SendInputFailed {
                sent,
                expected: inputs.len() as u32,
                error_code: err.0,
            });
        }
        Ok(())
    }

    /// Гарантированный сброс всех зажатых клавиш и кнопок мыши при обрыве соединения или завершении сессии
    pub fn release_all(&self) {
        let mut inputs = Vec::new();

        // 1. Отпускаем все зажатые кнопки мыши
        let held_buttons: Vec<MouseButton> = {
            let mut guard = self.pressed_mouse_buttons.lock().unwrap();
            guard.drain().collect()
        };

        for button in held_buttons {
            let (flag, data) = match button {
                MouseButton::Left => (MOUSEEVENTF_LEFTUP, 0),
                MouseButton::Right => (MOUSEEVENTF_RIGHTUP, 0),
                MouseButton::Middle => (MOUSEEVENTF_MIDDLEUP, 0),
                MouseButton::X1 => (MOUSEEVENTF_XUP, XBUTTON1),
                MouseButton::X2 => (MOUSEEVENTF_XUP, XBUTTON2),
            };
            inputs.push(INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: data,
                        dwFlags: flag,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }

        // Страховочный сброс базовых кнопок мыши
        for flag in [MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_MIDDLEUP] {
            inputs.push(INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: flag,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }

        // 2. Отпускаем все реально зажатые клавиши
        let held_keys: Vec<(u16, bool)> = {
            let mut guard = self.pressed_keys.lock().unwrap();
            guard.drain().collect()
        };

        for (scan_code, is_extended) in held_keys {
            let mut dw_flags = KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP;
            if is_extended {
                dw_flags |= KEYEVENTF_EXTENDEDKEY;
            }
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: scan_code,
                        dwFlags: dw_flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }

        // Страховочный сброс системных модификаторов: LCtrl, RCtrl, LAlt, RAlt, LShift, RShift, LWin, RWin
        let modifier_scancodes = [
            (0x1D, false),
            (0x1D, true),
            (0x38, false),
            (0x38, true),
            (0x2A, false),
            (0x36, false),
            (0x5B, true),
            (0x5C, true),
        ];

        for (sc, is_ext) in modifier_scancodes {
            let mut dw_flags = KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP;
            if is_ext {
                dw_flags |= KEYEVENTF_EXTENDEDKEY;
            }
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: sc,
                        dwFlags: dw_flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }

        let _ = self.send(&inputs);
    }
}

impl Default for WindowsInputInjector {
    fn default() -> Self {
        Self::new()
    }
}
