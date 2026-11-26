//! masterkeysd: owns the SteelSeries Apex 3 and serves lighting control on the session bus.

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("masterkeysd is the Linux service; on Windows run masterkeys.exe");
    std::process::ExitCode::FAILURE
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> std::process::ExitCode {
    use masterkeys_core::dbus::{BUS_NAME, OBJECT_PATH};
    use masterkeys_core::profile::ProfileStore;
    use masterkeysd::app::Daemon;
    use masterkeysd::persist::flush;
    use std::process::ExitCode;
    use tokio::signal::unix::{SignalKind, signal};
    use tracing::{error, info};
    use tracing_subscriber::EnvFilter;
    use zbus::fdo::{RequestNameFlags, RequestNameReply};

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let Some(config_dir) = std::env::var_os("MASTERKEYS_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(ProfileStore::default_config_dir)
    else {
        error!("cannot find a config directory (HOME is not set)");
        return ExitCode::FAILURE;
    };
    let daemon = Daemon::load(&config_dir);
    let (store, profiles) = (daemon.store.clone(), daemon.profiles.clone());
    let conn = match zbus::connection::Builder::session().serve_at(OBJECT_PATH, daemon.service()).build().await {
        Ok(conn) => conn,
        Err(e) => {
            error!("cannot start D-Bus service: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Claim the name without queueing, so a second instance fails instead of waiting in line.
    match conn.request_name_with_flags(BUS_NAME, RequestNameFlags::DO_NOT_QUEUE).await {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {}
        Ok(_) | Err(zbus::Error::NameTaken) => {
            eprintln!("masterkeysd is already running");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            error!("cannot claim {BUS_NAME}: {e}");
            return ExitCode::FAILURE;
        }
    }
    info!("serving {BUS_NAME} at {OBJECT_PATH}");

    daemon.spawn_tasks(conn.clone(), true);

    let (mut term, mut int) = match (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) {
        (Ok(t), Ok(i)) => (t, i),
        _ => {
            error!("cannot install signal handlers");
            return ExitCode::FAILURE;
        }
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
    // Save any edits still waiting out the save delay. Exit without touching the keyboard, so it keeps
    // showing the last frame.
    flush(&store, &profiles);
    info!("shutting down");
    ExitCode::SUCCESS
}
