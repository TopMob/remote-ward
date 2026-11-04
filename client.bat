@echo off
chcp 65001 >nul
title Remote-Ward Client
echo [Remote-Ward] Проверка обновлений...
git pull --ff-only

set HOST_IP=%1
if "%HOST_IP%"=="" set HOST_IP=192.168.1.4

echo [Remote-Ward] Подключение к хосту %HOST_IP%...
cargo run --release -p client-app -- %HOST_IP%
if %ERRORLEVEL% neq 0 (
    echo [Remote-Ward] Клиент остановлен.
    pause
)
