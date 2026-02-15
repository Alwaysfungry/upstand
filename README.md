# Upstand

Upstand is a local-first desktop app that helps users reduce long sitting streaks with reminder nudges and a clear 24-hour sit/stand dashboard.

## Version

Current release: **v1.2.0**

## Core Features

- Reminder intervals: 5 / 10 / 20 / 30 / 50 minutes
- Desktop reminder card with one-click acknowledgment
- Activity Insights dashboard with 24-hour heatmap
- Daily / Weekly / Monthly data ranges
- Export to CSV and Heatmap PNG
- Day / Night mode
- Local-only data handling (no cloud upload)

## Download (GitHub Releases)

Recommended files to publish in each release:

- `Upstand_1.2.0_x64-setup.exe` (primary installer)
- `Upstand_1.2.0_x64_en-US.msi` (enterprise/alternative installer)
- `upstand.exe` (portable manual run; optional)

Example latest-release links (replace placeholders):

- `https://github.com/<YOUR_USERNAME>/<YOUR_REPO>/releases/latest/download/Upstand_1.2.0_x64-setup.exe`
- `https://github.com/<YOUR_USERNAME>/<YOUR_REPO>/releases/latest/download/Upstand_1.2.0_x64_en-US.msi`

## Build From Source (Windows)

Prerequisites:

- Rust toolchain (`rustup`)
- Node.js + npm
- Tauri v2 Windows build dependencies

Build:

```powershell
npm install
npx tauri build
```

Build output:

- `target/release/upstand.exe`
- `target/release/bundle/nsis/Upstand_1.2.0_x64-setup.exe`
- `target/release/bundle/msi/Upstand_1.2.0_x64_en-US.msi`

## What To Upload To GitHub (Complete, practical set)

Keep in the repository:

- `Cargo.toml`
- `Cargo.lock`
- `tauri.conf.json`
- `build.rs`
- `src/`
- `dist/`
- `icons/`
- `README.md`
- `CHANGELOG.md`
- `LICENSE`
- `landing/`

Do **not** commit build output folders:

- `target/`
- `node_modules/` (if you can regenerate locally)

## Landing Page Template

A one-page landing template is included:

- `landing/index.html`
- `landing/assets/mockup-dashboard.svg`
- `landing/assets/mockup-interval.svg`
- `landing/assets/mockup-notification.svg`
- `landing/assets/mockup-export.svg`

## License

MIT. See `LICENSE`.
