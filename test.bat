@echo off
rem ============================================================
rem  test.bat — запуск unit-тестов Rust
rem
rem  Что делает:
rem    - Загружает окружение MSVC
rem    - Запускает "cargo test" в папке src-tauri
rem    - Показывает результат каждого теста (ok / FAILED)
rem
rem  Используй после изменений в Rust-коде чтобы убедиться,
rem  что ничего не сломалось.
rem ============================================================
echo [1/2] Loading MSVC environment...
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
set PATH=%USERPROFILE%\.cargo\bin;%PATH%

echo [2/2] Running tests...
cd /d "%~dp0src-tauri"
cargo test
