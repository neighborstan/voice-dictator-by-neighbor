@echo off
rem ============================================================
rem  dev.bat — запуск приложения в режиме разработки
rem
rem  Что делает:
rem    - Загружает окружение MSVC (нужно для компиляции Rust на Windows)
rem    - Запускает "npm run tauri dev":
rem        * поднимает Svelte dev-сервер (frontend)
rem        * компилирует Rust-бэкенд (первый раз ~5-10 мин)
rem        * открывает окно приложения
rem
rem  Используй каждый раз когда хочешь запустить и потестировать
rem  приложение вживую. Изменения в Svelte применяются горячо (hot reload),
rem  изменения в Rust требуют пересборки (авто, но занимает несколько секунд).
rem ============================================================
echo [1/2] Loading MSVC environment...
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
set PATH=%USERPROFILE%\.cargo\bin;%PATH%

set RUST_LOG=voice_dictator_lib=debug

echo [2/2] Starting Tauri dev server...
cd /d "%~dp0"
npm run tauri dev
