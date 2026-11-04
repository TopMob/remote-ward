@echo off
chcp 65001 >nul
title Remote-Ward
echo [Remote-Ward] Проверка обновлений...
git pull --ff-only
echo [Remote-Ward] Запуск приложения Remote-Ward...
cargo run --release -p client-app
if %ERRORLEVEL% neq 0 (
    echo [Remote-Ward] Приложение остановлено.
    pause
)
