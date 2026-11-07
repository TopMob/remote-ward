@echo off
setlocal enabledelayedexpansion
chcp 65001 >nul
title Remote-Ward Production Packaging Script

cd /d "%~dp0\.."
set "ROOT_DIR=%CD%"
set "DIST_DIR=%ROOT_DIR%\dist\remote-ward-win64"

echo ===================================================
echo     Remote-Ward Production Release Packaging
echo ===================================================
echo [1/5] Building workspace in release mode...
cargo build --release --workspace
if %ERRORLEVEL% neq 0 (
    echo [ERROR] Release build failed!
    pause
    exit /b 1
)

echo [2/5] Preparing distribution folder: %DIST_DIR%
if exist "%DIST_DIR%" rd /s /q "%DIST_DIR%"
mkdir "%DIST_DIR%"

echo [3/5] Copying binaries and runtime assets...
set "SRC_BIN_DIR=%ROOT_DIR%\target\x86_64-pc-windows-msvc\release"
if not exist "%SRC_BIN_DIR%\client-app.exe" (
    set "SRC_BIN_DIR=%ROOT_DIR%\target\release"
)

copy /y "%SRC_BIN_DIR%\client-app.exe" "%DIST_DIR%\" >nul
copy /y "%SRC_BIN_DIR%\host-server.exe" "%DIST_DIR%\" >nul
copy /y "%ROOT_DIR%\launcher.bat" "%DIST_DIR%\" >nul
copy /y "%ROOT_DIR%\host.bat" "%DIST_DIR%\" >nul
copy /y "%ROOT_DIR%\remote_ward_config.json" "%DIST_DIR%\" >nul
copy /y "%ROOT_DIR%\readme.md" "%DIST_DIR%\" >nul

echo [4/5] Verifying packaged binaries...
powershell -NoProfile -Command ^
    "$c = (Get-Item '%DIST_DIR%\client-app.exe').Length / 1MB; " ^
    "$h = (Get-Item '%DIST_DIR%\host-server.exe').Length / 1MB; " ^
    "$c_hash = (Get-FileHash '%DIST_DIR%\client-app.exe' -Algorithm SHA256).Hash; " ^
    "$h_hash = (Get-FileHash '%DIST_DIR%\host-server.exe' -Algorithm SHA256).Hash; " ^
    "Write-Host ('client-app.exe:  {0:N2} MB  [SHA256: {1}]' -f $c, $c_hash); " ^
    "Write-Host ('host-server.exe: {0:N2} MB  [SHA256: {1}]' -f $h, $h_hash);"

echo [5/5] Creating release zip archive...
powershell -NoProfile -Command ^
    "Compress-Archive -Path '%DIST_DIR%\*' -DestinationPath '%ROOT_DIR%\dist\remote-ward-win64.zip' -Force; " ^
    "Write-Host 'Created: dist\remote-ward-win64.zip'"

echo ===================================================
echo Release packaging completed successfully!
echo Files located in: %ROOT_DIR%\dist\
echo ===================================================
