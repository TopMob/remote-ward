@echo off
chcp 65001 >nul
title Remote-Ward Host Server
echo [Remote-Ward] Проверка обновлений...
git pull --ff-only
cls
echo [Remote-Ward] Запуск хост-сервера (NVENC + WGC + WASAPI)...
cargo run --release -p host-server
if %ERRORLEVEL% neq 0 (
    echo [Remote-Ward] Хост-сервер остановлен с ошибкой.
    pause
)
