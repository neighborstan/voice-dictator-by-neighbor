@echo off
rem ============================================================
rem  build.bat — сборка portable-версии для Windows
rem
rem  Что делает:
rem    1. Загружает окружение MSVC
rem    2. Собирает Tauri-приложение в release-режиме (без инсталлятора)
rem    3. Создаёт один портативный exe в dist/:
rem       Voice Dictator-<version>-win-x64-Portable.exe
rem
rem  Модель Silero VAD вшита в бинарник — никаких лишних файлов.
rem  Результат: один exe-файл, никакой установки не нужно.
rem ============================================================
echo [1/4] Loading MSVC environment...
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
set PATH=%USERPROFILE%\.cargo\bin;%PATH%
cd /d "%~dp0"

echo [2/4] Reading version from tauri.conf.json...
for /f "usebackq delims=" %%V in (`powershell -NoProfile -Command "(Get-Content src-tauri\tauri.conf.json | ConvertFrom-Json).version"`) do set APP_VERSION=%%V

if "%APP_VERSION%"=="" (
    echo ERROR: Could not read version from tauri.conf.json
    exit /b 1
)
echo Version: %APP_VERSION%

echo [3/4] Building release — no installer (this may take a few minutes)...
call npm run tauri build -- --no-bundle

set SRC_EXE=src-tauri\target\release\voice-dictator.exe
if not exist "%SRC_EXE%" (
    echo.
    echo BUILD FAILED — exe not found: %SRC_EXE%
    pause
    exit /b 1
)

echo [4/4] Copying portable exe...
set DIST_DIR=dist
set DEST=%DIST_DIR%\Voice Dictator-%APP_VERSION%-win-x64-Portable.exe

if not exist "%SRC_EXE%" (
    echo ERROR: %SRC_EXE% not found
    exit /b 1
)

if not exist "%DIST_DIR%" mkdir "%DIST_DIR%"
copy /Y "%SRC_EXE%" "%DEST%" >nul
if %ERRORLEVEL% NEQ 0 (
    echo ERROR: Failed to copy exe
    exit /b 1
)

echo.
echo === Build complete ===
echo.
echo File: %DEST%
echo Full path: %~dp0%DEST%
echo.
pause
