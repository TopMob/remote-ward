@echo off
setlocal enabledelayedexpansion
chcp 65001 >nul
title Remote-Ward Client

:: Always set working directory to script location
cd /d "%~dp0"

echo ===================================================
echo           Remote-Ward Launcher
echo ===================================================

:: Check for git updates if git is available and repo exists
where git >nul 2>&1
if %ERRORLEVEL% equ 0 (
    if exist ".git" (
        echo [Remote-Ward] Checking for repository updates...
        git pull --ff-only 2>nul
        if !ERRORLEVEL! equ 0 (
            echo [Remote-Ward] Repository is up to date.
        ) else (
            echo [Remote-Ward] Notice: Offline mode or working tree has local changes; skipping auto-update.
        )
    )
)

:: Locate release binary
set "BIN_PATH="
if exist "client-app.exe" (
    set "BIN_PATH=client-app.exe"
) else if exist "target\x86_64-pc-windows-msvc\release\client-app.exe" (
    set "BIN_PATH=target\x86_64-pc-windows-msvc\release\client-app.exe"
) else if exist "target\release\client-app.exe" (
    set "BIN_PATH=target\release\client-app.exe"
)

:: If binary does not exist, build it on-demand
if not defined BIN_PATH (
    echo [Remote-Ward] Release binary not found. Building release target...
    where cargo >nul 2>&1
    if !ERRORLEVEL! neq 0 (
        echo [Remote-Ward] ERROR: Neither prebuilt binary nor Rust/Cargo toolchain was found!
        echo Please install Rust from https://rustup.rs/ or download prebuilt release binaries.
        pause
        exit /b 1
    )
    cargo build --release -p client-app
    if !ERRORLEVEL! neq 0 (
        echo [Remote-Ward] ERROR: Build failed.
        pause
        exit /b 1
    )
    if exist "target\x86_64-pc-windows-msvc\release\client-app.exe" (
        set "BIN_PATH=target\x86_64-pc-windows-msvc\release\client-app.exe"
    ) else if exist "target\release\client-app.exe" (
        set "BIN_PATH=target\release\client-app.exe"
    )
)

echo [Remote-Ward] Starting Remote-Ward Client (%BIN_PATH%)...
"%BIN_PATH%" %*
set "APP_EXIT_CODE=%ERRORLEVEL%"

if %APP_EXIT_CODE% neq 0 (
    echo [Remote-Ward] Application exited with error code: %APP_EXIT_CODE%
    pause
)
exit /b %APP_EXIT_CODE%
