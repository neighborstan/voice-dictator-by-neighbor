@echo off
rem ============================================================
rem  fmt.bat — автоматическое форматирование Rust-кода
rem
rem  Что делает:
rem    - Загружает окружение MSVC
rem    - Запускает "cargo fmt" — форматирует все .rs файлы
rem      по стандарту rustfmt (отступы, пробелы, переносы строк)
rem
rem  Используй если check-all.bat ругается на форматирование,
rem  или просто перед коммитом чтобы код был аккуратным.
rem  Изменения применяются к файлам автоматически.
rem ============================================================
echo Loading MSVC environment...
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
set PATH=%USERPROFILE%\.cargo\bin;%PATH%

cd /d "%~dp0src-tauri"
cargo fmt
echo Done.
