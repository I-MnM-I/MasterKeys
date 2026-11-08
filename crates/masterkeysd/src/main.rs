//! masterkeysd: owns the SteelSeries Apex 3 and serves lighting control on the session bus.

use masterkeys_core::LightingState;
use masterkeys_core::dbus::{BUS_NAME, OBJECT_PATH};
use masterkeysd::device::apex3_opener;
use masterkeysd::output::{self, Backoff, Shared};
use masterkeysd::service::{DaemonService, watch_connection};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::Color;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let (state_tx, state_rx) = watch::channel(LightingState::default());
    let (connected_tx, connected_rx) = watch::channel(false);
    let frame = Arc::new(Mutex::new([Color::BLACK; ZONE_COUNT]));

    let service = DaemonService::new(state_tx, connected_rx.clone(), frame.clone());
    let conn = match zbus::connection::Builder::session().serve_at(OBJECT_PATH, service).build().await {
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

    let shared = Shared { state: state_rx, connected: connected_tx, frame };
    tokio::spawn(output::run(apex3_opener(), shared, Backoff::DEFAULT));
    tokio::spawn(watch_connection(conn.clone(), connected_rx));

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
    // Exit without touching the keyboard, so it keeps showing the last frame.
    info!("shutting down");
    ExitCode::SUCCESS
}
