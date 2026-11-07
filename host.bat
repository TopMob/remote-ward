@echo off
setlocal enabledelayedexpansion
chcp 65001 >nul
title Remote-Ward Host Server

:: Always set working directory to script location
cd /d "%~dp0"

echo ===================================================
echo         Remote-Ward Headless Host Server
echo ===================================================

:: Locate release binary
set "BIN_PATH="
if exist "host-server.exe" (
    set "BIN_PATH=host-server.exe"
) else if exist "target\x86_64-pc-windows-msvc\release\host-server.exe" (
    set "BIN_PATH=target\x86_64-pc-windows-msvc\release\host-server.exe"
) else if exist "target\release\host-server.exe" (
    set "BIN_PATH=target\release\host-server.exe"
)

:: If binary does not exist, build it on-demand
if not defined BIN_PATH (
    echo [Remote-Ward] Release host binary not found. Building release target...
    where cargo >nul 2>&1
    if !ERRORLEVEL! neq 0 (
        echo [Remote-Ward] ERROR: Neither prebuilt binary nor Rust/Cargo toolchain was found!
        echo Please install Rust from https://rustup.rs/ or download prebuilt release binaries.
        pause
        exit /b 1
    )
    cargo build --release -p host-server
    if !ERRORLEVEL! neq 0 (
        echo [Remote-Ward] ERROR: Build failed.
        pause
        exit /b 1
    )
    if exist "target\x86_64-pc-windows-msvc\release\host-server.exe" (
        set "BIN_PATH=target\x86_64-pc-windows-msvc\release\host-server.exe"
    ) else if exist "target\release\host-server.exe" (
        set "BIN_PATH=target\release\host-server.exe"
    )
)

echo [Remote-Ward] Starting Remote-Ward Host Server (%BIN_PATH%)...
"%BIN_PATH%" %*
set "APP_EXIT_CODE=%ERRORLEVEL%"

if %APP_EXIT_CODE% neq 0 (
    echo [Remote-Ward] Host server stopped with error code: %APP_EXIT_CODE%
    pause
)
exit /b %APP_EXIT_CODE%
