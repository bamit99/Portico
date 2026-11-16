use portico_core::daemon::PorticoDaemon;
use portico_jni::{attach_usb_fd, init_fd_usb_context};
use tempfile::TempDir;

#[tokio::test]
async fn fd_usb_context_initializes_and_invalid_fd_returns_error_without_panic() {
    let ctx = init_fd_usb_context().expect("init_fd_usb_context should succeed");

    let dir = TempDir::new().unwrap();
    let daemon = PorticoDaemon::start_with_bind_overrides(
        dir.path().to_path_buf(),
        dir.path().join("portico-ctrl.sock"),
        Some("127.0.0.1:0".parse().unwrap()),
        Some("127.0.0.1:0".parse().unwrap()),
    )
    .await
    .unwrap();

    let res = attach_usb_fd(
        &daemon.handle(),
        &ctx,
        -1,
        "1-99",
        1,
        99,
        Some("InvalidFD"),
    )
    .await;
    assert!(res.is_err());

    daemon.shutdown().await;
}
