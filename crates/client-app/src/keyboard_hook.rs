//! # Windows Low-Level Keyboard Hook (WH_KEYBOARD_LL)
//!
//! Перехватывает системные клавиши Windows (клавиши Win, Alt+Tab, Ctrl+Esc,
//! системные шорткаты Win+D, Win+E, Win+R, Win+Tab, Alt+F4 и т.д.), которые операционная система
//! клиента обычно перехватывает до того, как они дойдут до окна приложения.
//!
//! При активном окне удаленного сеанса:
//! 1. Клавиша отправляется на хост через `input_tx`.
//! 2. Локальная обработка блокируется (возврат 1 из хука), чтобы клиентский ПК (ноутбук)
//!    не открывал меню Пуск, не сворачивал окна и не переключал локальные задачи.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, MAPVK_VK_TO_VSC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetAncestor, GetForegroundWindow,
    GetMessageW, GetWindowThreadProcessId, PostMessageW, PostThreadMessageW,
    SetWindowsHookExW, UnhookWindowsHookEx, GA_ROOT, HHOOK, KBDLLHOOKSTRUCT, MSG,
    WH_KEYBOARD_LL, WM_CLOSE, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use core_protocol::{ButtonState, InputEvent};

const VK_TAB: u32 = 0x09;
const VK_ESCAPE: u32 = 0x1B;
const VK_SPACE: u32 = 0x20;
const VK_CONTROL: i32 = 0x11;
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_MENU: i32 = 0x12; // Alt
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
const VK_END: u32 = 0x23;
const VK_F4: u32 = 0x73;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;

// Флаг в KBDLLHOOKSTRUCT.flags: нажата клавиша Alt
const LLKHF_ALTDOWN: u32 = 0x20;
const LLKHF_EXTENDED: u32 = 0x01;

/// Атомарное отслеживание состояния системных модификаторов в памяти хука
static WIN_DOWN: AtomicBool = AtomicBool::new(false);
static ALT_DOWN: AtomicBool = AtomicBool::new(false);
static CTRL_DOWN: AtomicBool = AtomicBool::new(false);

/// Глобальное состояние для обратного вызова хука Windows
struct HookContext {
    target_hwnd: HWND,
    is_active: Arc<AtomicBool>,
    is_running: Arc<AtomicBool>,
    input_tx: mpsc::UnboundedSender<InputEvent>,
}

static mut HOOK_HANDLE: HHOOK = HHOOK(std::ptr::null_mut());
static mut HOOK_CTX: Option<HookContext> = None;

#[inline]
fn resolve_scancode(kbd: &KBDLLHOOKSTRUCT) -> u16 {
    let raw = (kbd.scanCode & 0xFF) as u16;
    if raw != 0 {
        raw
    } else {
        unsafe { MapVirtualKeyW(kbd.vkCode, MAPVK_VK_TO_VSC) as u16 }
    }
}

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
                let fg = GetForegroundWindow();
                let mut fg_pid = 0u32;
                GetWindowThreadProcessId(fg, Some(&mut fg_pid));
                let current_pid = windows::Win32::System::Threading::GetCurrentProcessId();

                let is_target_focused = (fg_pid != 0 && fg_pid == current_pid)
                    || fg == ctx.target_hwnd
                    || {
                        let fg_root = GetAncestor(fg, GA_ROOT);
                        let target_root = GetAncestor(ctx.target_hwnd, GA_ROOT);
                        fg_root == target_root && fg_root != HWND(std::ptr::null_mut())
                    };

                if is_target_focused {
                    let kbd = &*(l_param.0 as *const KBDLLHOOKSTRUCT);
                    let vk = kbd.vkCode;
                    let flags = kbd.flags.0;
                    let is_extended = (flags & LLKHF_EXTENDED) != 0;
                    let alt_flag = (flags & LLKHF_ALTDOWN) != 0;

                    let msg_type = w_param.0 as u32;
                    let is_down = msg_type == WM_KEYDOWN || msg_type == WM_SYSKEYDOWN;
                    let is_up = msg_type == WM_KEYUP || msg_type == WM_SYSKEYUP;

                    if is_down || is_up {
                        let state = if is_down {
                            ButtonState::Pressed
                        } else {
                            ButtonState::Released
                        };

                        // Обновляем состояния модификаторов в памяти хука
                        if vk == VK_LMENU || vk == VK_RMENU || vk == VK_MENU as u32 {
                            ALT_DOWN.store(is_down, Ordering::Relaxed);
                        }
                        if vk == VK_LCONTROL || vk == VK_RCONTROL || vk == VK_CONTROL as u32 {
                            CTRL_DOWN.store(is_down, Ordering::Relaxed);
                        }

                        // 0. ХОСТ-КЛАВИША: Right Control
                        let rctrl_down = vk == VK_RCONTROL || (GetAsyncKeyState(VK_RCONTROL as i32) as u16 & 0x8000) != 0;
                        if rctrl_down {
                            // Right Control + Esc: экстренное чистое закрытие клиента
                            if vk == VK_ESCAPE {
                                if is_down {
                                    tracing::info!("[KEY_HOOK] Нажат Right Ctrl + Esc -> экстренное завершение клиента");
                                    ctx.is_running.store(false, Ordering::Relaxed);
                                    let _ = PostMessageW(ctx.target_hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                                }
                                return LRESULT(1);
                            }

                            // Right Control + H: освобождение мыши и выход из полноэкранного режима
                            if vk == 0x48 /* Key 'H' */ {
                                return CallNextHookEx(HOOK_HANDLE, n_code, w_param, l_param);
                            }
                        }

                        // 1. Комбинация Ctrl + Alt + End -> отправка Ctrl+Alt+Del на хост
                        let alt_pressed = alt_flag || ALT_DOWN.load(Ordering::Relaxed) || (GetAsyncKeyState(VK_MENU) as u16 & 0x8000) != 0;
                        let ctrl_pressed = CTRL_DOWN.load(Ordering::Relaxed) || (GetAsyncKeyState(VK_CONTROL) as u16 & 0x8000) != 0;

                        if vk == VK_END && alt_pressed && ctrl_pressed {
                            tracing::info!("[KEY_HOOK] Перехвачен Ctrl+Alt+End -> отправка Ctrl+Alt+Del на хост");
                            let _ = ctx.input_tx.send(InputEvent::Keyboard {
                                scan_code: 0x53, // Scan code Delete
                                is_extended: true,
                                state,
                            });
                            return LRESULT(1);
                        }

                        // 2. Системные клавиши Win (Пуск) и комбинации (Win+D, Win+E, Win+R, Win+Tab, Win+1..9 и т.д.)
                        if vk == VK_LWIN || vk == VK_RWIN {
                            WIN_DOWN.store(is_down, Ordering::Relaxed);
                            let scan_code = if vk == VK_LWIN { 0x5B } else { 0x5C };
                            let _ = ctx.input_tx.send(InputEvent::Keyboard {
                                scan_code,
                                is_extended: true,
                                state,
                            });
                            // Блокируем локальное открытие меню "Пуск" на клиенте!
                            return LRESULT(1);
                        }

                        if WIN_DOWN.load(Ordering::Relaxed) {
                            // При зажатом Win перехватываем ЛЮБУЮ клавишу (Win+D, Win+E, Win+R, Win+Tab, Win+Стрелки...)
                            let scan_code = resolve_scancode(kbd);
                            let _ = ctx.input_tx.send(InputEvent::Keyboard {
                                scan_code,
                                is_extended,
                                state,
                            });
                            // Блокируем локальное выполнение (сворачивание окон по Win+D на клиенте)
                            return LRESULT(1);
                        }

                        // 3. Alt + Tab, Alt + Shift + Tab, Alt + Esc, Alt + Space, Alt + F4
                        if alt_pressed {
                            if vk == VK_TAB {
                                let _ = ctx.input_tx.send(InputEvent::Keyboard {
                                    scan_code: 0x0F, // Scan code Tab
                                    is_extended: false,
                                    state,
                                });
                                // Блокируем локальный Alt+Tab переключатель задач на клиенте!
                                return LRESULT(1);
                            }

                            if vk == VK_ESCAPE {
                                let _ = ctx.input_tx.send(InputEvent::Keyboard {
                                    scan_code: 0x01, // Esc
                                    is_extended: false,
                                    state,
                                });
                                return LRESULT(1);
                            }

                            if vk == VK_SPACE {
                                let _ = ctx.input_tx.send(InputEvent::Keyboard {
                                    scan_code: 0x39, // Space
                                    is_extended: false,
                                    state,
                                });
                                return LRESULT(1);
                            }

                            if vk == VK_F4 {
                                let _ = ctx.input_tx.send(InputEvent::Keyboard {
                                    scan_code: 0x3E, // F4
                                    is_extended: false,
                                    state,
                                });
                                // Передаем Alt+F4 на хост для закрытия активного окна на ПК, не закрывая окно клиента
                                return LRESULT(1);
                            }
                        }

                        // 4. Ctrl + Esc (еще один системный способ открыть Пуск на хосте)
                        if vk == VK_ESCAPE && ctrl_pressed && !rctrl_down {
                            let _ = ctx.input_tx.send(InputEvent::Keyboard {
                                scan_code: 0x01, // Esc
                                is_extended: false,
                                state,
                            });
                            return LRESULT(1);
                        }
                    }
                }
            } else {
                // Если окно не активно, сбрасываем состояние зажатых модификаторов
                WIN_DOWN.store(false, Ordering::Relaxed);
                ALT_DOWN.store(false, Ordering::Relaxed);
                CTRL_DOWN.store(false, Ordering::Relaxed);
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
        is_running: Arc<AtomicBool>,
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
                    is_running,
                    input_tx,
                });

                let hmod = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
                let hook = SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(low_level_keyboard_proc),
                    hmod,
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
                WIN_DOWN.store(false, Ordering::Relaxed);
                ALT_DOWN.store(false, Ordering::Relaxed);
                CTRL_DOWN.store(false, Ordering::Relaxed);
                tracing::info!("Глобальный хук WH_KEYBOARD_LL снят");
            })
            .expect("Failed to spawn keyboard-hook thread");

        Self {
            thread_id,
            is_active,
        }
    }

    /// Обновить канал передачи событий ввода на активную сессию стрима
    pub fn update_sender(&self, input_tx: mpsc::UnboundedSender<InputEvent>) {
        unsafe {
            if let Some(ref mut ctx) = HOOK_CTX {
                ctx.input_tx = input_tx;
            }
        }
    }

    /// Включить/выключить перехват (например, при потере фокуса)
    pub fn set_active(&self, active: bool) {
        if !active {
            WIN_DOWN.store(false, Ordering::Relaxed);
            ALT_DOWN.store(false, Ordering::Relaxed);
            CTRL_DOWN.store(false, Ordering::Relaxed);
        }
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
