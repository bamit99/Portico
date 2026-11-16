# Android App (`portico-jni` + `android/`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the JNI bridge (`crates/portico-jni`) and the Android Kotlin + Jetpack Compose (Material 3) app (`android/`, `io.portico.app`) wrapping the Rust core daemon with root-free `UsbManager` fd sharing, `ForegroundService`, `BootReceiver`, `WifiLock`/`WakeLock`, and all 11 parity menu features.

**Architecture:** `crates/portico-jni` compiles to `libportico_jni.so` (`arm64-v8a` for `OnePlus 12R`, plus `x86_64` for emulator/host tests) and embeds `portico-core`. Before creating any `rusb::Context`, `init_android_usb()` calls `rusb::disable_device_discovery()` (the proven README recipe). Kotlin `PorticoService` hosts the daemon, manages `UsbDeviceConnection` lifecycles, binds a `ControlChannelClient` over `<filesDir>/portico-ctrl.sock`, and drives a reactive Compose UI via `StateFlow`.

**Tech Stack:** Rust `jni` crate + `rusb` (`vendored` feature) + `cargo-ndk`, Android SDK (`compileSdk 35/36`, `minSdk 26`, `targetSdk 35`), Kotlin, Jetpack Compose BOM + Material 3, Gradle 8.9.

**Spec:** `DESIGN.md` (§2, §3, §4, §5, §8), `FEATURES.md` (all sections), `README.md` (§Android glue).

## Global Constraints

- Package ID: `io.portico.app`.
- Root-free Android USB path ONLY: `UsbManager.requestPermission` -> `UsbDeviceConnection.getFileDescriptor()` -> JNI -> `rusb::disable_device_discovery()` + `Context::open_device_with_fd(fd)`.
- `UsbDeviceConnection` MUST stay open in Kotlin as long as the device is registered in Rust (`libusb_wrap_sys_device` does NOT take fd ownership; closing `UsbDeviceConnection` invalidates the fd).
- `WifiManager.WifiLock` (`WIFI_MODE_FULL_HIGH_PERF` / `WIFI_MODE_FULL_LOW_LATENCY`) and `PowerManager.WakeLock` (`PARTIAL_WAKE_LOCK`) released cleanly in `onDestroy()`.
- All 11 menu items + status text + ignored devices picker `product name(vvpp)` + reverse clients + TLS/LAN warning + claim token view/regen implemented.
- Clean-room UI: brand-new Portico naming, icons, Material 3 layout, wording.

## Review Focus

1. Closing `UsbDeviceConnection` in Kotlin before Rust finishes `DETACH_DEVICE` (or unplugging while `in_use`) — must call `nativeDetachUsb(busId)` before `connection.close()` to avoid EBADF/use-after-close on fd. (Pinned in Task 1 & Task 3)
2. `rusb::disable_device_discovery()` called after a `rusb::Context` was already constructed — must run exactly once at process/daemon init via `Once` before any `Context::new()`. (Pinned in Task 1)
3. `BootReceiver` starting foreground service on Android 12+ (`API 31+`) — must use `Context.startForegroundService()` and immediately call `startForeground()` within 5 seconds, only when `start_on_boot` is persisted `true`. (Pinned in Task 3)
4. `Ignored Devices` dialog receiving invalid hex input (`"123"`, `"GGGG/1234"`) or empty checkbox removal — must show validation toast and leave persisted list unchanged. (Pinned in Task 5)
5. `TLS & Security` disabling TLS when user has not confirmed the plaintext-LAN risk dialog — switch must remain ON unless warning dialog is explicitly accepted. (Pinned in Task 6)

---

### Task 1: `crates/portico-jni` Crate & Android FD-Wrapping USB Attachment

**Files:**
- Create: `crates/portico-jni/Cargo.toml`
- Create: `crates/portico-jni/src/lib.rs`
- Modify: `Cargo.toml` (add `crates/portico-jni` to workspace)
- Test: `crates/portico-jni/tests/fd_attach_tests.rs`

**Interfaces:**
- Consumes: `portico_core::{PorticoDaemon, usb_device_from_rusb_handle}`, `rusb::{disable_device_discovery, Context, UsbContext}`
- Produces JNI exports for `io.portico.app.nativebridge.PorticoNative`:
  - `nativeStartDaemon(dataDir: String, ctrlSockPath: String): Boolean`
  - `nativeStopDaemon(): Unit`
  - `nativeAttachUsbFd(fd: Int, busId: String, busNum: Int, devNum: Int, fallbackProduct: String): Boolean`
  - `nativeDetachUsb(busId: String): Boolean`
  - `nativeExecControl(cmd: String): String`

- [ ] **Step 1: Create `crates/portico-jni/Cargo.toml` & `src/lib.rs` with `Once` `disable_device_discovery()` guard**
- [ ] **Step 2: Write unit test in `crates/portico-jni/tests/fd_attach_tests.rs`**
  Test `init_fd_usb_context()` initializes `rusb::Context` with `disable_device_discovery()`, and `attach_usb_fd` gracefully returns an error (no panic) on an invalid fd (`-1`) instead of crashing the JVM process.
- [ ] **Step 3: Run `cargo test -p portico-jni` (RED -> GREEN)**
- [ ] **Step 4: Commit**
  `git add Cargo.toml crates/portico-jni && git commit -m "feat(jni): portico-jni crate with disable_device_discovery and JNI exports"`

---

### Task 2: Android Gradle Project Scaffold & JNI Native Loader (`io.portico.app`)

**Files:**
- Create: `android/settings.gradle.kts`
- Create: `android/build.gradle.kts`
- Create: `android/gradle.properties`
- Create: `android/app/build.gradle.kts`
- Create: `android/app/src/main/AndroidManifest.xml`
- Create: `android/app/src/main/java/io/portico/app/nativebridge/PorticoNative.kt`
- Create: `android/scripts/build_jni.sh`

**Interfaces:**
- Produces: compilable Kotlin + Jetpack Compose Android Gradle project targeting `compileSdk 35`, `minSdk 26`, `targetSdk 35`, bundling `libportico_jni.so` for `arm64-v8a`.
- Manifest permissions: `INTERNET`, `ACCESS_NETWORK_STATE`, `ACCESS_WIFI_STATE`, `CHANGE_WIFI_MULTICAST_STATE`, `WAKE_LOCK`, `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_CONNECTED_DEVICE`, `RECEIVE_BOOT_COMPLETED`, `POST_NOTIFICATIONS`.

- [ ] **Step 1: Verify/install Android NDK via `sdkmanager` if missing and `aarch64-linux-android` rustup target**
- [ ] **Step 2: Write Gradle project files, Manifest, `PorticoNative.kt`, and `build_jni.sh`**
- [ ] **Step 3: Compile `libportico_jni.so` (`arm64-v8a`) and run `./gradlew :app:compileDebugKotlin` to verify build**
- [ ] **Step 4: Commit**
  `git add android && git commit -m "build(android): Gradle Compose scaffold and arm64-v8a JNI build script"`

---

### Task 3: `PorticoService` (ForegroundService), `BootReceiver`, Locks, & `UsbPermissionController`

**Files:**
- Create: `android/app/src/main/java/io/portico/app/service/PorticoService.kt`
- Create: `android/app/src/main/java/io/portico/app/service/BootReceiver.kt`
- Create: `android/app/src/main/java/io/portico/app/usb/UsbDeviceManagerBridge.kt`
- Create: `android/app/src/main/java/io/portico/app/control/ControlChannelClient.kt`
- Test: `android/app/src/test/java/io/portico/app/ControlChannelParserTest.kt`
- Test: `android/app/src/test/java/io/portico/app/BootPreferenceTest.kt`

**Interfaces:**
- `ControlChannelClient` speaks `portico-ctrl` line protocol over `android.net.LocalSocket` (`LocalSocketAddress.Namespace.FILESYSTEM`, `<filesDir>/portico-ctrl.sock`) + `SUBSCRIBE` event reader loop.
- `UsbDeviceManagerBridge`:
  - Enumerates `UsbManager.deviceList`, checks `hasPermission(dev)`.
  - Requests permission via `PendingIntent` + `BroadcastReceiver` (`ACTION_USB_PERMISSION`).
  - Handles `ACTION_USB_DEVICE_ATTACHED` / `ACTION_USB_DEVICE_DETACHED` broadcasts.
  - Opens `UsbDeviceConnection`, passes `connection.fileDescriptor` to `PorticoNative.nativeAttachUsbFd(...)`, keeps `UsbDeviceConnection` in a `ConcurrentHashMap<String, UsbDeviceConnection>`, and closes it only AFTER `PorticoNative.nativeDetachUsb(busId)`.

- [ ] **Step 1: Write unit tests `ControlChannelParserTest.kt` and `BootPreferenceTest.kt` (RED)**
- [ ] **Step 2: Run `./gradlew :app:testDebugUnitTest` to verify RED**
- [ ] **Step 3: Implement `ControlChannelClient.kt`, `UsbDeviceManagerBridge.kt`, `PorticoService.kt`, `BootReceiver.kt`**
- [ ] **Step 4: Run `./gradlew :app:testDebugUnitTest` to verify GREEN**
- [ ] **Step 5: Commit**
  `git add android && git commit -m "feat(android): ForegroundService, BootReceiver, USB fd bridge, and control client"`

---

### Task 4: Material 3 Theme, `PorticoViewModel`, & Main Status/Devices Screen

**Files:**
- Create: `android/app/src/main/java/io/portico/app/ui/theme/PorticoTheme.kt`
- Create: `android/app/src/main/java/io/portico/app/ui/PorticoViewModel.kt`
- Create: `android/app/src/main/java/io/portico/app/ui/MainScreen.kt`
- Create: `android/app/src/main/java/io/portico/app/MainActivity.kt`
- Test: `android/app/src/test/java/io/portico/app/ViewModelStateTest.kt`

**Interfaces:**
- Main screen displays:
  - Top app bar with status pill (`Sharing` / `Paused`), Start/Stop button, overflow menu (11 items).
  - Status text (`"USB devices plugged into this phone are shared over your network once you grant USB access."`) + `"Desktop client setup"` sheet/link.
  - Security card (TLS badge or LAN plaintext warning banner, server fingerprint + copy, claim token reveal/copy/regen).
  - Relay status label (`"Currently disabled"` / `"Please wait, initializing…"` / `"Available — enter <addr> in any client to connect!"`).
  - Attached USB devices list with status pills & `Grant Access` / `Ignore` actions.

- [ ] **Step 1: Write `ViewModelStateTest.kt` testing `GET_STATE` / `LIST_DEVICES` / `EVENT` state mapping (RED)**
- [ ] **Step 2: Implement `PorticoTheme.kt`, `PorticoViewModel.kt`, `MainScreen.kt`, `MainActivity.kt` (GREEN)**
- [ ] **Step 3: Run `./gradlew :app:testDebugUnitTest`**
- [ ] **Step 4: Commit**
  `git add android && git commit -m "feat(android): Material 3 theme, ViewModel, and main status/devices screen"`

---

### Task 5: Ignored Devices, Reverse Clients, & System Log Dialogs/Screens

**Files:**
- Create: `android/app/src/main/java/io/portico/app/ui/IgnoredDevicesSheet.kt`
- Create: `android/app/src/main/java/io/portico/app/ui/ReverseClientsSheet.kt`
- Create: `android/app/src/main/java/io/portico/app/ui/SystemLogSheet.kt`
- Create: `android/app/src/main/java/io/portico/app/ui/Validation.kt`
- Test: `android/app/src/test/java/io/portico/app/InputValidationTest.kt`

**Interfaces:**
- `Validation.parseVidPid(input: String): String?` -> returns normalized `"VVVV/PPPP"` or null.
- `Validation.formatAttachedPickerItem(product: String, vidHex: String, pidHex: String): String` -> `"$product($vidHex$pidHex)"`.
- `Validation.parseReverseClient(addr: String, fp: String): Pair<String, String>?`.
- Checkbox multi-select remove with toast if zero selected; toast on invalid input.

- [ ] **Step 1: Write `InputValidationTest.kt` covering valid/invalid VID/PID, picker formatting, and reverse client validation (RED)**
- [ ] **Step 2: Implement `Validation.kt`, `IgnoredDevicesSheet.kt`, `ReverseClientsSheet.kt`, `SystemLogSheet.kt` (GREEN)**
- [ ] **Step 3: Run `./gradlew :app:testDebugUnitTest`**
- [ ] **Step 4: Commit**
  `git add android && git commit -m "feat(android): Ignored Devices, Reverse Clients, and System Log UI"`

---

### Task 6: Relay Settings, TLS/Security Settings, Help, About, Exit, & APK Build/Verification

**Files:**
- Create: `android/app/src/main/java/io/portico/app/ui/SecuritySettingsSheet.kt`
- Create: `android/app/src/main/java/io/portico/app/ui/RelaySettingsSheet.kt`
- Create: `android/app/src/main/java/io/portico/app/ui/InfoDialogs.kt`
- Modify: `android/app/src/main/java/io/portico/app/ui/MainScreen.kt`

- [ ] **Step 1: Implement `SecuritySettingsSheet.kt` (with plaintext LAN risk confirmation dialog), `RelaySettingsSheet.kt`, `InfoDialogs.kt` (Help, About, Client Setup)**
- [ ] **Step 2: Wire all 11 overflow menu actions in `MainScreen.kt` (including `Exit` calling `SHUTDOWN` + `stopSelf()` + `finishAndRemoveTask()`)**
- [ ] **Step 3: Build debug APK `./gradlew :app:assembleDebug` and run `:app:testDebugUnitTest` + `:app:lintDebug`**
- [ ] **Step 4: If `OnePlus 12R` is connected via `adb`, install APK, launch, grant USB permission on a real USB device, and verify `portico list` + `portico attach` from desktop!**
- [ ] **Step 5: Commit**
  `git add android && git commit -m "feat(android): Security/Relay/Info dialogs and APK build"`
