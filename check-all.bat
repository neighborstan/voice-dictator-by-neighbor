@echo off
rem ============================================================
rem  check-all.bat — полная проверка качества кода перед коммитом
rem
rem  Что делает (по порядку, останавливается при первой ошибке):
rem    1. cargo check  — проверяет компиляцию без сборки бинарника
rem    2. cargo clippy — линтер: ищет ошибки и плохой код (с -D warnings)
rem    3. cargo test   — запускает все unit-тесты
rem    4. cargo fmt    — проверяет форматирование кода
rem
rem  Используй перед каждым коммитом. Если fmt упал — запусти fmt.bat.
rem ============================================================
echo Loading MSVC environment...
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
set PATH=%USERPROFILE%\.cargo\bin;%PATH%
cd /d "%~dp0src-tauri"

echo.
echo === [1/4] cargo check ===
cargo check
if %ERRORLEVEL% NEQ 0 ( echo FAILED & exit /b 1 )
echo OK

echo.
echo === [2/4] cargo clippy ===
cargo clippy --all-targets -- -D warnings
if %ERRORLEVEL% NEQ 0 ( echo FAILED & exit /b 1 )
echo OK

echo.
echo === [3/4] cargo test ===
cargo test
if %ERRORLEVEL% NEQ 0 ( echo FAILED & exit /b 1 )
echo OK

echo.
echo === [4/4] cargo fmt --check ===
cargo fmt --check
if %ERRORLEVEL% NEQ 0 (
    echo FAILED - run fmt.bat to fix
    exit /b 1
)
echo OK

echo.
echo === All checks passed ===
