//! # Windows Low-Level Keyboard Hook (WH_KEYBOARD_LL)
//!
//! Перехватывает системные клавиши Windows (клавиши Win, Alt+Tab, Ctrl+Esc,
//! системные шорткаты Win+D, Win+E, Win+R и т.д.), которые операционная система
//! клиента обычно перехватывает до того, как они дойдут до окна приложения.
//!
//! При активном окне удаленного сеанса:
//! 1. Клавиша отправляется на хост через `input_tx`.
//! 2. Локальная обработка блокируется (возврат 1 из хука), чтобы клиентский ПК
//!    не открывал меню Пуск и не переключал локальные окна.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use core_protocol::{ButtonState, InputEvent};

const VK_TAB: u32 = 0x09;
const VK_ESCAPE: u32 = 0x1B;
const VK_CONTROL: i32 = 0x11;
const VK_END: u32 = 0x23;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;

// Флаг в KBDLLHOOKSTRUCT.flags: нажата клавиша Alt
const LLKHF_ALTDOWN: u32 = 0x20;
const LLKHF_EXTENDED: u32 = 0x01;

/// Глобальное состояние для обратного вызова хука Windows
struct HookContext {
    target_hwnd: HWND,
    is_active: Arc<AtomicBool>,
    input_tx: mpsc::UnboundedSender<InputEvent>,
}

static mut HOOK_HANDLE: HHOOK = HHOOK(std::ptr::null_mut());
static mut HOOK_CTX: Option<HookContext> = None;

/// Процедура хука WH_KEYBOARD_LL
unsafe extern "system" fn low_level_keyboard_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code >= 0 {
        if let Some(ref ctx) = HOOK_CTX {
            // Проверяем: включен ли захват и активно ли наше окно
            if ctx.is_active.load(Ordering::Relaxed) {
                let foreground = GetForegroundWindow();
                if foreground == ctx.target_hwnd {
                    let kbd = &*(l_param.0 as *const KBDLLHOOKSTRUCT);
                    let vk = kbd.vkCode;
                    let flags = kbd.flags.0;
                    let _is_extended = (flags & LLKHF_EXTENDED) != 0;
                    let alt_down = (flags & LLKHF_ALTDOWN) != 0;

                    let msg_type = w_param.0 as u32;
                    let is_down = msg_type == WM_KEYDOWN || msg_type == WM_SYSKEYDOWN;
                    let is_up = msg_type == WM_KEYUP || msg_type == WM_SYSKEYUP;

                    if is_down || is_up {
                        let state = if is_down {
                            ButtonState::Pressed
                        } else {
                            ButtonState::Released
                        };

                        // 1. Проверяем комбинацию Ctrl + Alt + End (шорткат для Ctrl+Alt+Del на удаленном хосте)
                        if vk == VK_END && alt_down {
                            let ctrl_down = (GetAsyncKeyState(VK_CONTROL) as u16 & 0x8000) != 0;
                            if ctrl_down {
                                // Эмулируем Ctrl + Alt + Del на хосте (нажимаем и отпускаем Del с зажатыми Ctrl+Alt)
                                tracing::info!("[KEY_HOOK] Перехвачен Ctrl+Alt+End -> отправка Ctrl+Alt+Del на хост");
                                let _ = ctx.input_tx.send(InputEvent::Keyboard {
                                    scan_code: 0x53, // Scan code Delete
                                    is_extended: true,
                                    state,
                                });
                                return LRESULT(1);
                            }
                        }

                        // 2. Клавиши Win (левая и правая)
                        if vk == VK_LWIN || vk == VK_RWIN {
                            let scan_code = if vk == VK_LWIN { 0x5B } else { 0x5C };
                            let _ = ctx.input_tx.send(InputEvent::Keyboard {
                                scan_code,
                                is_extended: true,
                                state,
                            });
                            // Блокируем локальное открытие меню "Пуск" на клиенте!
                            return LRESULT(1);
                        }

                        // 3. Alt + Tab (переключение окон на хосте, а не на клиенте!)
                        if vk == VK_TAB && alt_down {
                            let _ = ctx.input_tx.send(InputEvent::Keyboard {
                                scan_code: 0x0F, // Scan code Tab
                                is_extended: false,
                                state,
                            });
                            // Блокируем локальный Alt+Tab переключатель задач на клиенте!
                            return LRESULT(1);
                        }

                        // 4. Ctrl + Esc (еще один системный способ открыть Пуск)
                        if vk == VK_ESCAPE {
                            let ctrl_down = (GetAsyncKeyState(VK_CONTROL) as u16 & 0x8000) != 0;
                            if ctrl_down {
                                let _ = ctx.input_tx.send(InputEvent::Keyboard {
                                    scan_code: 0x01, // Esc
                                    is_extended: false,
                                    state,
                                });
                                return LRESULT(1);
                            }
                        }
                    }
                }
            }
        }
    }

    CallNextHookEx(HOOK_HANDLE, n_code, w_param, l_param)
}

/// Управляемый сторож хука клавиатуры
pub struct SystemKeyboardHook {
    thread_id: Arc<AtomicU32>,
    is_active: Arc<AtomicBool>,
}

impl SystemKeyboardHook {
    /// Установить низкоуровневый хук клавиатуры Windows в выделенном потоке
    pub fn install(
        target_hwnd: HWND,
        input_tx: mpsc::UnboundedSender<InputEvent>,
    ) -> Self {
        let is_active = Arc::new(AtomicBool::new(true));
        let thread_id = Arc::new(AtomicU32::new(0));

        let is_active_clone = Arc::clone(&is_active);
        let thread_id_clone = Arc::clone(&thread_id);

        let target_raw = target_hwnd.0 as isize;

        std::thread::Builder::new()
            .name("keyboard-hook".into())
            .spawn(move || unsafe {
                let tid = windows::Win32::System::Threading::GetCurrentThreadId();
                thread_id_clone.store(tid, Ordering::SeqCst);

                HOOK_CTX = Some(HookContext {
                    target_hwnd: HWND(target_raw as *mut _),
                    is_active: is_active_clone,
                    input_tx,
                });

                let hook = SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(low_level_keyboard_proc),
                    HMODULE(std::ptr::null_mut()),
                    0,
                );

                match hook {
                    Ok(h) => {
                        HOOK_HANDLE = h;
                        tracing::info!("Глобальный хук WH_KEYBOARD_LL успешно установлен");
                    }
                    Err(e) => {
                        tracing::error!("Не удалось установить WH_KEYBOARD_LL: {:?}", e);
                        return;
                    }
                }

                // Цикл сообщений Windows (обязателен для функционирования LL хуков)
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).as_bool() {
                    if msg.message == WM_QUIT {
                        break;
                    }
                    DispatchMessageW(&msg);
                }

                let _ = UnhookWindowsHookEx(HOOK_HANDLE);
                HOOK_HANDLE = HHOOK(std::ptr::null_mut());
                HOOK_CTX = None;
                tracing::info!("Глобальный хук WH_KEYBOARD_LL снят");
            })
            .expect("Failed to spawn keyboard-hook thread");

        Self {
            thread_id,
            is_active,
        }
    }

    /// Включить/выключить перехват (например, при потере фокуса)
    pub fn set_active(&self, active: bool) {
        self.is_active.store(active, Ordering::SeqCst);
    }
}

impl Drop for SystemKeyboardHook {
    fn drop(&mut self) {
        let tid = self.thread_id.load(Ordering::SeqCst);
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}
