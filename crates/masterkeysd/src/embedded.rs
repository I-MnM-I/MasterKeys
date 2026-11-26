//! The core inside the Windows app: the daemon on a runtime thread of its own, reached over a private
//! peer-to-peer connection instead of the session bus.

use crate::app::Daemon;
use std::path::PathBuf;

/// Start the core on the profiles in `config_dir`, driving the lighting devices it finds; returns the
/// client end of its connection.
pub fn start(config_dir: PathBuf) -> zbus::Result<zbus::Connection> {
    start_with(config_dir, true)
}

/// `start`, with `device` deciding whether lighting hardware is driven (tests pass false).
pub fn start_with(config_dir: PathBuf, device: bool) -> zbus::Result<zbus::Connection> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("masterkeys-core".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(r) => r,
                Err(e) => return drop(tx.send(Err(zbus::Error::Failure(format!("tokio runtime: {e}"))))),
            };
            runtime.block_on(async move {
                let daemon = Daemon::load(&config_dir);
                match crate::p2p::pair(daemon.service()).await {
                    Ok((server, client)) => {
                        daemon.spawn_tasks(server, device);
                        let _ = tx.send(Ok(client));
                        std::future::pending::<()>().await
                    }
                    Err(e) => drop(tx.send(Err(e))),
                }
            });
        })
        .map_err(|e| zbus::Error::Failure(format!("core thread: {e}")))?;
    rx.recv().map_err(|_| zbus::Error::Failure("the core thread stopped".into()))?
}
