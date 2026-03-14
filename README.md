# Voice Dictator

[![version](https://img.shields.io/badge/version-0.1.2-blue)](CHANGELOG.md#012)
[![license](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)](https://tauri.app)
[![tauri](https://img.shields.io/badge/built%20with-Tauri%20v2-24C8DB)](https://tauri.app)
[![rust](https://img.shields.io/badge/rust-1.88%2B-orange)](https://www.rust-lang.org)

Tray-first десктопное приложение для голосовой диктовки.

Нажал хоткей - говоришь - нажал снова - текст вставляется в активное поле (IDE, браузер, мессенджер).
Транскрипция через OpenAI API, опциональное улучшение текста через LLM.
Работает из системного трея, без лишних окон.

## Возможности

- Два режима записи: Toggle (нажал-говоришь-нажал) и Push-to-Talk (удержание)
- Онлайн STT через OpenAI API - модель настраивается
- Улучшение текста через LLM: пунктуация, грамматика (отключается)
- Автоматическая вставка через clipboard с восстановлением предыдущего содержимого
- VAD auto-stop: запись останавливается при тишине
- Настройки через UI: хоткей, язык, модели STT и LLM, параметры записи
- Онбординг при первом запуске
- Уведомления ОС при смене состояния

## Стек

- **Tauri v2 + Rust** - бэкенд, системная интеграция
- **Svelte / SvelteKit** - UI настроек
- **OpenAI API** - STT (Whisper) + улучшение текста (Responses API)
- **cpal** - захват аудио (WASAPI / CoreAudio / ALSA)
- **enigo + arboard** - симуляция вставки, clipboard

## Требования

- Windows 10+ (проверено), macOS / Linux X11 (в процессе)
- API-ключ OpenAI
- [Rust toolchain](https://rustup.rs/)
- Node.js 18+

## Запуск для разработки

```bash
npm install
npm run tauri dev
```

При первом запуске приложение откроет окно Settings - введи API-ключ OpenAI.

## Сборка

```bash
npm run build        # только фронтенд
cargo tauri build    # полный бандл (в src-tauri/)
```

## Лицензия

MIT