# Portico Progress

## Milestones
- [x] **M0 — Design & Plans**: `DESIGN.md` + 4 subsystem plans (`plans/protocol-security.md`, `plans/core-daemon.md`, `plans/desktop-cli.md`, `plans/android-app.md`)
- [x] **M1 — Core Daemon + Protocol/Security** (`portico-proto`, `portico-relay`, `portico-core`) — 25/25 tests green + code review fixes (`afa112f`)
- [x] **M2 — Desktop CLI** (`portico-cli`) — `discover`, `trust`, `list`, `attach`, `detach`, `listen`, `identity`, `serve`, `relay` — 29/29 workspace tests green + code review fixes (`6e08ede`)
- [x] **M3 — Android GUI + JNI** (`portico-jni`, `android/`) — `libportico_jni.so` (`arm64-v8a`), `app-debug.apk` built, 30/30 Rust workspace tests + 9/9 Android JVM unit tests + `lintDebug` green (`423497a`)
- [ ] **M4 — Full E2E Polish & On-Device Acceptance** (`OnePlus 12R` + real USB device) — **Blocked on hardware**: no Android device detected via `adb devices`

## Log
- **2026-11-16**: Initialized `DESIGN.md` (with builder-check resolution) and `plans/` for all 4 subsystems.
- **2026-11-16**: Completed M1 (`portico-proto`, `portico-relay`, `portico-core`) with TDD + code review fixes (`afa112f`). Note: `rusb` currently uses `vendored` feature on this host because system `libusb-1.0-dev` headers are absent; dynamic-link packaging should be revisited before binary release per README LGPL note.
- **2026-11-16**: Completed M2 (`portico-cli`) with TDD + code review fixes (`6e08ede`).
- **2026-11-16**: Completed M3 (`portico-jni`, `android/`) with NDK r29 `arm64-v8a` build, Compose Material 3 app (`io.portico.app`), code review fixes (`423497a`), and APK build (`android/app/build/outputs/apk/debug/app-debug.apk`).
- **2026-11-16**: Stopped at M4 on-device acceptance: `adb devices -l` lists 0 attached devices.
