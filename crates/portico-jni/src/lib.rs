use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, jstring, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;
use portico_core::daemon::{DaemonHandle, PorticoDaemon};
use portico_core::registry::DeviceRecord;
use portico_core::usb_handler::usb_device_from_rusb_handle;
use portico_core::CoreError;
use rusb::UsbContext;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tokio::runtime::Runtime;
use tracing::error;

static DISCOVERY_DISABLED: OnceLock<Result<(), String>> = OnceLock::new();

pub fn init_fd_usb_context() -> Result<rusb::Context, CoreError> {
    #[cfg(unix)]
    {
        // Only cache success; retry if an earlier call failed
        if DISCOVERY_DISABLED.get().is_none() {
            if let Err(e) = rusb::disable_device_discovery() {
                return Err(CoreError::Usb(format!(
                    "disable_device_discovery failed: {e}"
                )));
            }
            let _ = DISCOVERY_DISABLED.set(Ok(()));
        }
    }
    rusb::Context::new().map_err(|e| CoreError::Usb(format!("rusb Context::new: {e}")))
}

pub async fn attach_usb_fd(
    daemon: &DaemonHandle,
    ctx: &rusb::Context,
    fd: i32,
    bus_id: &str,
    bus_num: u32,
    dev_num: u32,
    fallback_product: Option<&str>,
) -> Result<DeviceRecord, CoreError> {
    if fd < 0 {
        return Err(CoreError::Usb(format!("invalid usb fd: {fd}")));
    }
    #[cfg(unix)]
    let handle = unsafe { ctx.open_device_with_fd(fd) }
        .map_err(|e| CoreError::Usb(format!("open_device_with_fd({fd}): {e}")))?;
    #[cfg(not(unix))]
    return Err(CoreError::Usb("fd attachment requires unix".into()));

    let (speed_setting, ignored) = {
        let cfg = daemon.state.config.lock().await;
        (cfg.config.usb_speed, cfg.config.ignored_devices.clone())
    };
    let (usb_dev, mfr, prod, ser) = usb_device_from_rusb_handle(
        handle,
        bus_id,
        bus_num,
        dev_num,
        speed_setting,
        fallback_product,
    )?;

    daemon
        .state
        .registry
        .register_usb_device(usb_dev, mfr, prod, ser, &ignored)
        .await
}

struct DaemonSlot {
    daemon: PorticoDaemon,
    handle: DaemonHandle,
    usb_ctx: rusb::Context,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static DAEMON_SLOT: OnceLock<Mutex<Option<DaemonSlot>>> = OnceLock::new();

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

fn slot_mutex() -> &'static Mutex<Option<DaemonSlot>> {
    DAEMON_SLOT.get_or_init(|| Mutex::new(None))
}

fn clone_handle_and_ctx() -> Option<(DaemonHandle, rusb::Context)> {
    let guard = slot_mutex().lock().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .map(|s| (s.handle.clone(), s.usb_ctx.clone()))
}

fn clone_handle() -> Option<DaemonHandle> {
    let guard = slot_mutex().lock().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().map(|s| s.handle.clone())
}

fn get_jstring(env: &mut JNIEnv, s: JString) -> Option<String> {
    if s.is_null() {
        return None;
    }
    env.get_string(&s).ok().map(|js| js.into())
}

#[no_mangle]
pub extern "system" fn Java_io_portico_app_nativebridge_PorticoNative_nativeStartDaemon(
    mut env: JNIEnv,
    _class: JClass,
    data_dir: JString,
    ctrl_sock_path: JString,
) -> jboolean {
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(data_dir) = get_jstring(&mut env, data_dir) else {
            return false;
        };
        let Some(ctrl_sock) = get_jstring(&mut env, ctrl_sock_path) else {
            return false;
        };
        if clone_handle().is_some() {
            return true;
        }
        let ctx = match init_fd_usb_context() {
            Ok(c) => c,
            Err(e) => {
                error!("init_fd_usb_context: {e}");
                return false;
            }
        };
        let daemon = match runtime().block_on(PorticoDaemon::start(
            PathBuf::from(data_dir),
            PathBuf::from(ctrl_sock),
        )) {
            Ok(d) => d,
            Err(e) => {
                error!("PorticoDaemon::start: {e}");
                return false;
            }
        };
        let handle = daemon.handle();
        let mut guard = slot_mutex().lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            drop(guard);
            runtime().block_on(daemon.shutdown());
            return true;
        }
        *guard = Some(DaemonSlot {
            daemon,
            handle,
            usb_ctx: ctx,
        });
        true
    }));
    if matches!(res, Ok(true)) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_io_portico_app_nativebridge_PorticoNative_nativeStopDaemon(
    _env: JNIEnv,
    _class: JClass,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let taken = {
            let mut guard = slot_mutex().lock().unwrap_or_else(|e| e.into_inner());
            guard.take()
        };
        if let Some(slot) = taken {
            runtime().block_on(slot.daemon.shutdown());
        }
    }));
}

#[no_mangle]
pub extern "system" fn Java_io_portico_app_nativebridge_PorticoNative_nativeAttachUsbFd(
    mut env: JNIEnv,
    _class: JClass,
    fd: jint,
    bus_id: JString,
    bus_num: jint,
    dev_num: jint,
    fallback_product: JString,
) -> jboolean {
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(bus_id) = get_jstring(&mut env, bus_id) else {
            return false;
        };
        let fallback = get_jstring(&mut env, fallback_product);
        let Some((handle, ctx)) = clone_handle_and_ctx() else {
            return false;
        };
        runtime()
            .block_on(attach_usb_fd(
                &handle,
                &ctx,
                fd,
                &bus_id,
                bus_num.max(0) as u32,
                dev_num.max(0) as u32,
                fallback.as_deref(),
            ))
            .is_ok()
    }));
    if matches!(res, Ok(true)) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_io_portico_app_nativebridge_PorticoNative_nativeRegisterPendingUsb(
    mut env: JNIEnv,
    _class: JClass,
    bus_id: JString,
    vid: jint,
    pid: jint,
    product: JString,
) -> jboolean {
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(bus_id) = get_jstring(&mut env, bus_id) else {
            return false;
        };
        let product = get_jstring(&mut env, product).unwrap_or_else(|| "USB Device".into());
        let Some(handle) = clone_handle() else {
            return false;
        };
        runtime().block_on(handle.state.registry.register_pending_permission(
            bus_id,
            vid as u16,
            pid as u16,
            product,
        ));
        true
    }));
    if matches!(res, Ok(true)) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_io_portico_app_nativebridge_PorticoNative_nativeIsVidPidIgnored(
    _env: JNIEnv,
    _class: JClass,
    vid: jint,
    pid: jint,
) -> jboolean {
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(handle) = clone_handle() else {
            return false;
        };
        let v = vid as u16;
        let p = pid as u16;
        runtime().block_on(async move {
            let cfg = handle.state.config.lock().await;
            cfg.config
                .ignored_devices
                .iter()
                .any(|vp| vp.vid == v && vp.pid == p)
        })
    }));
    if matches!(res, Ok(true)) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_io_portico_app_nativebridge_PorticoNative_nativeDetachUsb(
    mut env: JNIEnv,
    _class: JClass,
    bus_id: JString,
) -> jboolean {
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(bus_id) = get_jstring(&mut env, bus_id) else {
            return false;
        };
        let Some(handle) = clone_handle() else {
            return false;
        };
        runtime()
            .block_on(handle.state.registry.detach_device(&bus_id))
            .is_ok()
    }));
    if matches!(res, Ok(true)) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_io_portico_app_nativebridge_PorticoNative_nativeExecControl(
    mut env: JNIEnv,
    _class: JClass,
    cmd: JString,
) -> jstring {
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(cmd_str) = get_jstring(&mut env, cmd) else {
            return "ERR null_cmd".to_string();
        };
        let Some(handle) = clone_handle() else {
            return "ERR daemon_not_running".to_string();
        };
        runtime().block_on(handle.exec_command(&cmd_str))
    }))
    .unwrap_or_else(|_| "ERR panic".to_string());

    env.new_string(out)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
