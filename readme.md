# Удалённый доступ к ПК по локальной сети

Личный проект. В будущем возможно расширение на глобальную сеть.

## Термины

- **Хост** — удалённый ПК, к которому подключаются.
- **Клиент** — устройство, с которого подключаются.

## Цель и контекст

- **Тип проекта:** личный (на первом этапе).
- **Количество клиентов на один хост:** один. Одновременные подключения нескольких клиентов не нужны.
- **Дистрибуция:** нужен установщик (инсталлятор) для хоста и клиента.
- **Основной сценарий:** *(уточнить: игры / работа / администрирование)*
- **ОС хоста и клиента:** *(уточнить)*

## Целевые показатели

| Параметр | Целевое значение (черновик) |
|---|---|
| Задержка от ввода до картинки (LAN) | ≤ 30–50 мс |
| Частота кадров | 60 fps (опционально 120) |
| Максимальное разрешение | до 4K |
| Качество картинки | без заметных артефактов на тексте, режим 4:4:4 |
| Время подключения | ≤ 2 с |
| Нагрузка на CPU/GPU хоста | *(определить после прототипа)* |

## Основные функции

### 1. Картинка
- Минимальная задержка.
- Чёткая картинка (текст читается без артефактов).
- Поддержка нескольких мониторов хоста, выбор монитора для трансляции.
- Автоматическая подгонка под клиентское устройство: разрешение, масштаб (DPI), частота кадров.
- Режим «приоритет задержки / приоритет качества», адаптивный битрейт.

### 2. Ввод
- Полный ввод «как будто сижу за удалённым ПК»:
  - все комбинации клавиш (Ctrl+C, Ctrl+Win+V и т. д.);
  - перехват системных сочетаний;
  - режим захвата мыши (относительный для игр, абсолютный для работы).
- Синхронизация буфера обмена (текст, изображения, файлы — уточнить объём).
- Работа на экране входа и блокировки (UAC, Ctrl+Alt+Del).

### 3. Звук
- Звук хоста → клиенту.
- Микрофон клиента → хосту.
- Заглушение динамиков хоста при сохранении звука на клиенте (потребуется виртуальное аудиоустройство).

### 4. Управление хостом
- Выключение или затемнение физического монитора хоста во время сессии.
- Виртуальный монитор, если физический выключен или отсутствует.
- Блокировка локального ввода на хосте на время сессии.
- Запуск хоста как службы, автостарт, автоматическое переподключение.
- Wake-on-LAN.

### 5. Сеть и подключение
- Подключение по локальной сети.
- Автообнаружение хостов в локальной сети.
- Сохранение профилей подключений.

## Безопасность

- Сопряжение устройств (PIN или QR-код).
- Подтверждение первого подключения на хосте.
- Шифрование трафика (DTLS/TLS, ключи на устройство).
- Список доверенных клиентов.
- Определить поведение, когда на хосте никого нет (что может делать клиент).

> Обязательно проработать до выхода в глобальную сеть.

## Установка и распространение

- Установщик для хоста (с регистрацией службы, правами, правилами брандмауэра).
- Установщик или портативная сборка для клиента.
- Автоматическое обновление *(на потом)*.
- Деинсталляция без остатков (служба, драйверы виртуального аудио и монитора).

## Дорожная карта

| Этап | Содержание |
|---|---|
| **MVP** | Картинка + мышь/клавиатура + автообнаружение по LAN + базовый установщик |
| **v1** | Звук, буфер обмена, несколько мониторов, автоподгонка разрешения |
| **v2** | Затемнение монитора хоста, перехват системных сочетаний, передача файлов, Wake-on-LAN |
| **Позже** | Глобальная сеть (NAT traversal), мобильные клиенты, геймпад, HDR, запись сессии |

## Техническая часть (вопросы для проработки)

- **Платформы:** Windows / Linux / macOS для хоста; что для клиента, нужны ли Android/iOS?
- **Транспорт:** собственный протокол поверх UDP, QUIC или WebRTC (WebRTC даёт готовый NAT traversal для будущей глобальной сети).
- **Обнаружение:** mDNS/DNS-SD или UDP broadcast.
- **Захват экрана:** DXGI Desktop Duplication, Windows Graphics Capture, PipeWire.
- **Кодирование:** NVENC, AMF, QuickSync; кодеки H.264, H.265, AV1.
- **Перехват системных сочетаний:** low-level hooks в Windows и ограничения на других ОС.
- **Виртуальные устройства:** драйвер виртуального монитора и виртуального аудио.
- **Стек:** Rust, C++, Go или другое; нативный клиент или Electron.
- **Установщик:** MSI, Inno Setup, NSIS или аналог.
- **Аналоги для изучения:** Sunshine/Moonlight, Parsec, RustDesk, Apache Guacamole.


'PS C:\Users\TopMob\Documents\Projects\remote-ward> git fetch
PS C:\Users\TopMob\Documents\Projects\remote-ward> git init
Reinitialized existing Git repository in C:/Users/TopMob/Documents/Projects/remote-ward/.git/
PS C:\Users\TopMob\Documents\Projects\remote-ward> git commit -m "readme"
On branch main

Initial commit

Untracked files:
  (use "git add <file>..." to include in what will be committed)
        readme.md

nothing added to commit but untracked files present (use "git add" to track)
PS C:\Users\TopMob\Documents\Projects\remote-ward> git push
error: src refspec refs/heads/main does not match any
error: failed to push some refs to 'https://github.com/TopMob/remote-ward'
PS C:\Users\TopMob\Documents\Projects\remote-ward> git add -A
PS C:\Users\TopMob\Documents\Projects\remote-ward> git push  
error: src refspec refs/heads/main does not match any
error: failed to push some refs to 'https://github.com/TopMob/remote-ward'
PS C:\Users\TopMob\Documents\Projects\remote-ward> git commit -m "readme"
[main (root-commit) 2c90524] readme
 1 file changed, 100 insertions(+)
 create mode 100644 readme.md
PS C:\Users\TopMob\Documents\Projects\remote-ward> git push

 *  History restored 

PS C:\Users\TopMob\Documents\Projects\remote-ward> cargo run --release -p client-app -- 192.168.1.50
    Updating crates.io index
  Downloaded bytemuck v1.25.2
  Downloaded parking_lot v0.12.5
  Downloaded smol_str v0.2.2
  Downloaded bincode v1.3.3
  Downloaded thiserror v2.0.21
  Downloaded zmij v1.0.23
  Downloaded tracing-log v0.2.0
  Downloaded find-msvc-tools v0.1.14
  Downloaded tinyvec v1.13.3
  Downloaded rand_core v0.10.1
  Downloaded windows-interface v0.58.0
  Downloaded zeroize v1.9.0
  Downloaded windows-targets v0.52.6
  Downloaded tracing-attributes v0.1.31
  Downloaded sharded-slab v0.1.7
  Downloaded unicode-segmentation v1.13.3
  Downloaded quinn v0.11.12
  Downloaded mio v1.2.4
  Downloaded memchr v2.8.3
  Downloaded libm v0.2.16
  Downloaded aho-corasick v1.1.5
  Downloaded lock_api v0.4.14
  Downloaded bytes v1.12.1
  Downloaded serde_derive v1.0.229
  Downloaded tracing-subscriber v0.3.23
  Downloaded syn v3.0.6
  Downloaded windows_x86_64_msvc v0.52.6
  Downloaded serde_core v1.0.229
  Downloaded chacha20 v0.10.2
  Downloaded shlex v2.0.1
  Downloaded rustls-pki-types v1.15.1
  Downloaded quinn-udp v0.5.16
  Downloaded parking_lot_core v0.9.12
  Downloaded libc v0.2.190
  Downloaded winit v0.30.13
  Downloaded getrandom v0.4.3
  Downloaded getrandom v0.2.17
  Downloaded foldhash v0.2.0
  Downloaded windows-implement v0.58.0
  Downloaded unicode-ident v1.0.26
  Downloaded windows-core v0.58.0
  Downloaded thread_local v1.1.10
  Downloaded thiserror-impl v2.0.21
  Downloaded slab v0.4.12
  Downloaded regex-automata v0.4.18
  Downloaded rustls-webpki v0.103.15
  Downloaded regex-syntax v0.8.11
  Downloaded quote v1.0.47
  Downloaded dpi v0.1.2
  Downloaded windows-strings v0.1.0
  Downloaded windows-link v0.2.1
  Downloaded untrusted v0.9.0
  Downloaded tokio-macros v2.7.2
  Downloaded subtle v2.6.1
  Downloaded scopeguard v1.2.0
  Downloaded rustls v0.23.45
  Downloaded tokio v1.53.2
  Downloaded ring v0.17.14
  Downloaded tracing-core v0.1.36
  Downloaded tracing v0.1.44
  Downloaded syn v2.0.119
  Downloaded socket2 v0.6.5
  Downloaded smallvec v1.16.2
  Downloaded serde v1.0.229
  Downloaded quinn-proto v0.11.19
  Downloaded anyhow v1.0.104
  Downloaded portable-atomic v1.15.0
  Downloaded bitflags v2.13.2
  Downloaded rustls-platform-verifier v0.7.1
  Downloaded proc-macro2 v1.0.107
  Downloaded log v0.4.34
  Downloaded rustc-hash v2.1.3
  Downloaded rand_pcg v0.10.2
  Downloaded pin-project-lite v0.2.17
  Downloaded nu-ansi-term v0.50.3
  Downloaded lru-slab v0.1.3
  Downloaded jobserver v0.1.35
  Downloaded cursor-icon v1.2.0
  Downloaded cfg-if v1.0.5
  Downloaded cc v1.6.0
  Downloaded windows-result v0.2.0
  Downloaded siphasher v1.0.4
  Downloaded serde_json v1.0.151
  Downloaded cfg_aliases v0.2.2
  Downloaded bytemuck_derive v1.12.1
  Downloaded raw-window-handle v0.6.2
  Downloaded rand v0.10.3
  Downloaded once_cell v1.21.4
  Downloaded cpufeatures v0.3.1
  Downloaded matchers v0.2.0
  Downloaded lazy_static v1.5.1
  Downloaded fastbloom v0.17.0
  Downloaded windows-sys v0.52.0
  Downloaded windows v0.58.0
  Downloaded windows-sys v0.61.2
  Downloaded 95 crates (24.8MiB) in 10.43s (largest was `windows` at 9.3MiB)
   Compiling proc-macro2 v1.0.107
   Compiling quote v1.0.47
   Compiling unicode-ident v1.0.26
   Compiling cfg-if v1.0.5
   Compiling windows-link v0.2.1
   Compiling find-msvc-tools v0.1.14
   Compiling once_cell v1.21.4
   Compiling shlex v2.0.1
   Compiling windows-sys v0.61.2                                                                 
   Compiling cfg_aliases v0.2.2
   Compiling windows_x86_64_msvc v0.52.6                                                         
   Compiling log v0.4.34                                                                         
   Compiling pin-project-lite v0.2.17                                                            
   Compiling tracing-core v0.1.36                                                                
   Compiling getrandom v0.2.17
   Compiling rand_core v0.10.1                                                                   
   Compiling untrusted v0.9.0                                                                    
   Compiling cc v1.6.0                                                                           
   Compiling zeroize v1.9.0                                                                      
   Compiling serde_core v1.0.229
   Compiling windows-targets v0.52.6                                                             
   Compiling libm v0.2.16                                                                        
   Compiling rustls-pki-types v1.15.1                                                            
   Compiling getrandom v0.4.3                                                                    
   Compiling portable-atomic v1.15.0                                                             
   Compiling rustls v0.23.45                                                                     
   Compiling parking_lot_core v0.9.12                                                            
   Compiling thiserror v2.0.21
   Compiling smallvec v1.16.2
   Compiling syn v3.0.6                                                                          
   Compiling syn v2.0.119                                                                        
   Compiling serde v1.0.229                                                                      
   Compiling subtle v2.6.1                                                                       
   Compiling zmij v1.0.23                                                                        
   Compiling cpufeatures v0.3.1
   Compiling libc v0.2.190                                                                       
   Compiling scopeguard v1.2.0                                                                   
   Compiling lock_api v0.4.14                                                                    
   Compiling chacha20 v0.10.2                                                                    
   Compiling ring v0.17.14                                                                       
   Compiling quinn-udp v0.5.16                                                                   
   Compiling bytes v1.12.1                                                                       
   Compiling serde_json v1.0.151                                                                 
   Compiling siphasher v1.0.4                                                                    
   Compiling foldhash v0.2.0                                                                     
   Compiling fastbloom v0.17.0                                                                   
   Compiling rand v0.10.3                                                                        
   Compiling parking_lot v0.12.5
   Compiling windows-result v0.2.0                                                               
   Compiling rand_pcg v0.10.2                                                                    
   Compiling quinn v0.11.12                                                                      
   Compiling tinyvec v1.13.3
   Compiling memchr v2.8.3                                                                       
   Compiling regex-syntax v0.8.11                                                                
   Compiling lru-slab v0.1.3                                                                     
   Compiling rustc-hash v2.1.3                                                                   
   Compiling slab v0.4.12                                                                        
   Compiling itoa v1.0.18                                                                        
   Compiling windows-strings v0.1.0                                                              
   Compiling socket2 v0.6.5                                                                      
   Compiling mio v1.2.4                                                                          
   Compiling winit v0.30.13                                                                      
   Compiling thiserror-impl v2.0.21                                                              
   Compiling serde_derive v1.0.229                                                               
   Compiling tokio-macros v2.7.2                                                                 
   Compiling rustls-webpki v0.103.15                                                             
   Compiling bytemuck_derive v1.12.1                                                             
   Compiling tracing-attributes v0.1.31                                                          
   Compiling windows-interface v0.58.0                                                           
   Compiling bytemuck v1.25.2                                                                    
   Compiling windows-implement v0.58.0                                                           
   Compiling tokio v1.53.2                                                                       
   Compiling tracing v0.1.44                                                                     
   Compiling regex-automata v0.4.18                                                              
   Compiling anyhow v1.0.104                                                                     
   Compiling lazy_static v1.5.1                                                                  
   Compiling sharded-slab v0.1.7                                                                 
   Compiling windows-core v0.58.0                                                                
   Compiling nu-ansi-term v0.50.3                                                                
   Compiling tracing-log v0.2.0                                                                  
   Compiling windows-sys v0.52.0
   Compiling thread_local v1.1.10                                                                
   Compiling raw-window-handle v0.6.2                                                            
   Compiling smol_str v0.2.2                                                                     
   Compiling bitflags v2.13.2                                                                    
   Compiling bincode v1.3.3                                                                      
   Compiling dpi v0.1.2                                                                          
   Compiling cursor-icon v1.2.0                                                                  
   Compiling unicode-segmentation v1.13.3                                                        
   Compiling windows v0.58.0                                                                     
   Compiling core-protocol v0.1.0 (C:\Users\TopMob\Documents\Projects\remote-ward\crates\core-protocol)
   Compiling rustls-platform-verifier v0.7.1                                                     
   Compiling quinn-proto v0.11.19                                                                
   Compiling matchers v0.2.0
   Compiling tracing-subscriber v0.3.23                                                          
   Compiling core-transport v0.1.0 (C:\Users\TopMob\Documents\Projects\remote-ward\crates\core-transport)
   Compiling client-app v0.1.0 (C:\Users\TopMob\Documents\Projects\remote-ward\crates\client-app)
    Finished `release` profile [optimized] target(s) in 1m 46s
     Running `target\x86_64-pc-windows-msvc\release\client-app.exe 192.168.1.50`
2026-10-06T17:56:48.665351Z  INFO client_app::app: ============================================================
2026-10-06T17:56:48.665966Z  INFO client_app::app:            ЗАПУСК КЛИЕНТА REMOTE-WARD (STREAMING)
2026-10-06T17:56:48.666432Z  INFO client_app::app: ============================================================
2026-10-06T17:56:48.669225Z  INFO client_app::app: Сетевой видеосокет клиента открыт на порту 48000
2026-10-06T17:56:48.698443Z  INFO client_app::app: Отправлен ClientHello на хост 192.168.1.50:48001
2026-10-06T17:56:48.703913Z  INFO client_app::decoder: Аппаратный декодер Media Foundation MFT (H264) успешно настроен: 2560x1440

thread 'main' (4972) panicked at C:\Users\TopMob\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\winit-0.30.13\src\platform_impl\windows\window.rs:1174:17:
OleInitialize failed! Result was: `RPC_E_CHANGED_MODE`. Make sure other crates are not using multithreaded COM library on the same thread or disable drag and drop support.
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
error: process didn't exit successfully: `target\x86_64-pc-windows-msvc\release\client-app.exe 192.168.1.50` (exit code: 0xc0000409, STATUS_STACK_BUFFER_OVERRUN)
PS C:\Users\TopMob\Documents\Projects\remote-ward> cargo run --release -p client-app -- 192.168.1.50
    Finished `release` profile [optimized] target(s) in 0.43s
     Running `target\x86_64-pc-windows-msvc\release\client-app.exe 192.168.1.50`
2026-10-06T17:57:13.072959Z  INFO client_app::app: ============================================================
2026-10-06T17:57:13.073683Z  INFO client_app::app:            ЗАПУСК КЛИЕНТА REMOTE-WARD (STREAMING)
2026-10-06T17:57:13.074097Z  INFO client_app::app: ============================================================
2026-10-06T17:57:13.078264Z  INFO client_app::app: Сетевой видеосокет клиента открыт на порту 48000
2026-10-06T17:57:13.089082Z  INFO client_app::app: Отправлен ClientHello на хост 192.168.1.50:48001
2026-10-06T17:57:13.092796Z  INFO client_app::decoder: Аппаратный декодер Media Foundation MFT (H264) успешно настроен: 2560x1440

thread 'main' (2272) panicked at C:\Users\TopMob\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\winit-0.30.13\src\platform_impl\windows\window.rs:1174:17:
OleInitialize failed! Result was: `RPC_E_CHANGED_MODE`. Make sure other crates are not using multithreaded COM library on the same thread or disable drag and drop support.
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
error: process didn't exit successfully: `target\x86_64-pc-windows-msvc\release\client-app.exe 192.168.1.50` (exit code: 0xc0000409, STATUS_STACK_BUFFER_OVERRUN)
PS C:\Users\TopMob\Documents\Projects\remote-ward> '