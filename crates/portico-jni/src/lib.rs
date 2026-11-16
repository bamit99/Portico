use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, jstring, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;
use portico_core::daemon::{DaemonHandle, PorticoDaemon};
use portico_core::registry::DeviceRecord;
use portico_core::usb_handler::usb_device_from_rusb_handle;
use portico_core::CoreError;
use rusb::UsbContext;
use std::path::PathBuf;
use std::sync::{Mutex, Once, OnceLock};
use tokio::runtime::Runtime;
use tracing::error;

static DISCOVERY_DISABLED: Once = Once::new();

pub fn init_fd_usb_context() -> Result<rusb::Context, CoreError> {
    #[cfg(unix)]
    {
        let mut init_err: Option<rusb::Error> = None;
        DISCOVERY_DISABLED.call_once(|| {
            if let Err(e) = rusb::disable_device_discovery() {
                init_err = Some(e);
            }
        });
        if let Some(e) = init_err {
            return Err(CoreError::Usb(format!(
                "disable_device_discovery failed: {e}"
            )));
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

struct GlobalDaemon {
    rt: Runtime,
    daemon: Option<PorticoDaemon>,
    usb_ctx: Option<rusb::Context>,
}

static GLOBAL: OnceLock<Mutex<GlobalDaemon>> = OnceLock::new();

fn global_state() -> &'static Mutex<GlobalDaemon> {
    GLOBAL.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        Mutex::new(GlobalDaemon {
            rt,
            daemon: None,
            usb_ctx: None,
        })
    })
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
        let mut g = global_state().lock().unwrap();
        if g.daemon.is_some() {
            return true;
        }
        let ctx = match init_fd_usb_context() {
            Ok(c) => c,
            Err(e) => {
                error!("init_fd_usb_context: {e}");
                return false;
            }
        };
        match g.rt.block_on(PorticoDaemon::start(
            PathBuf::from(data_dir),
            PathBuf::from(ctrl_sock),
        )) {
            Ok(d) => {
                g.usb_ctx = Some(ctx);
                g.daemon = Some(d);
                true
            }
            Err(e) => {
                error!("PorticoDaemon::start: {e}");
                false
            }
        }
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
        let mut g = global_state().lock().unwrap();
        if let Some(d) = g.daemon.take() {
            g.rt.block_on(d.shutdown());
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
        let g = global_state().lock().unwrap();
        let (Some(daemon), Some(ctx)) = (g.daemon.as_ref(), g.usb_ctx.as_ref()) else {
            return false;
        };
        let handle = daemon.handle();
        let ctx = ctx.clone();
        g.rt
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
        let g = global_state().lock().unwrap();
        let Some(daemon) = g.daemon.as_ref() else {
            return false;
        };
        let handle = daemon.handle();
        g.rt.block_on(handle.state.registry.register_pending_permission(
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
pub extern "system" fn Java_io_portico_app_nativebridge_PorticoNative_nativeDetachUsb(
    mut env: JNIEnv,
    _class: JClass,
    bus_id: JString,
) -> jboolean {
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(bus_id) = get_jstring(&mut env, bus_id) else {
            return false;
        };
        let g = global_state().lock().unwrap();
        let Some(daemon) = g.daemon.as_ref() else {
            return false;
        };
        let handle = daemon.handle();
        g.rt
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
        let g = global_state().lock().unwrap();
        let Some(daemon) = g.daemon.as_ref() else {
            return "ERR daemon_not_running".to_string();
        };
        let handle = daemon.handle();
        g.rt.block_on(handle.exec_command(&cmd_str))
    }))
    .unwrap_or_else(|_| "ERR panic".to_string());

    env.new_string(out)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
