//! Talks to masterkeysd from a background thread with its own tokio runtime.

use futures_util::StreamExt;
use masterkeys_core::dbus::{BUS_NAME, DaemonProxy};
use masterkeys_core::bindings::Macro;
use masterkeys_core::profile::ProfileSummary;
use masterkeys_core::{StateReport, to_hex};
use std::collections::HashSet;
use std::time::Duration;
use steelseries_gg::rgb::{Color, Effect};
use tokio::sync::mpsc;

/// What the UI asks for.
#[derive(Clone, Debug)]
pub enum Command {
    SetAll(Color),
    SetZone(u8, Color),
    SetBrightness(u8),
    SetEffect(Effect),
    ActivateProfile(String),
    CreateProfile(String),
    DeleteProfile(String),
    RenameProfile(String, String),
    SetExecutables(String, Vec<String>),
    SetDefault(String),
    SetRemap(String, String),
    ClearRemap(String),
    SetMacro(String, Macro),
    ClearMacro(String),
    StartRecording,
    /// Stop recording and put the macro on this key.
    StopRecording(String),
    StartDaemon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Connected,
    KeyboardNotFound,
    DaemonNotRunning,
}

impl Status {
    pub fn text(self) -> &'static str {
        match self {
            Status::Connected => "Connected",
            Status::KeyboardNotFound => "Keyboard not found",
            Status::DaemonNotRunning => "Daemon not running",
        }
    }
}

/// What the UI is told.
#[derive(Clone, Debug)]
pub enum Event {
    Status(Status),
    /// The lighting, active profile, its bindings and what the daemon does with the keys.
    State(Box<StateReport>),
    Profiles(Vec<ProfileSummary>),
    Key(u16, bool),
    Error(String),
}

/// Start the client thread. It keeps (re)connecting to the daemon for the life of the program.
pub fn spawn(on_event: impl Fn(Event) + Send + Sync + 'static) -> mpsc::UnboundedSender<Command> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
        runtime.block_on(async move {
            loop {
                match zbus::Connection::session().await {
                    Ok(conn) if daemon_present(&conn).await => run_on(conn, &on_event, &mut rx).await,
                    _ => on_event(Event::Status(Status::DaemonNotRunning)),
                }
                wait_offline(&on_event, &mut rx, Duration::from_secs(1)).await;
            }
        });
    });
    tx
}

async fn daemon_present(conn: &zbus::Connection) -> bool {
    match zbus::fdo::DBusProxy::new(conn).await {
        Ok(dbus) => dbus.name_has_owner(BUS_NAME.try_into().expect("valid name")).await.unwrap_or(false),
        Err(_) => false,
    }
}

/// While the daemon is away: handle StartDaemon, refuse other commands, return after `delay`.
async fn wait_offline(
    on_event: &(impl Fn(Event) + ?Sized),
    commands: &mut mpsc::UnboundedReceiver<Command>,
    delay: Duration,
) {
    let deadline = tokio::time::sleep(delay);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return,
            command = commands.recv() => match command {
                Some(Command::StartDaemon) => start_daemon(on_event).await,
                Some(_) => on_event(Event::Error("the daemon is not running".into())),
                None => std::future::pending::<()>().await,
            },
        }
    }
}

async fn start_daemon(on_event: &(impl Fn(Event) + ?Sized)) {
    let result = tokio::process::Command::new("systemctl").args(["--user", "start", "masterkeysd"]).status().await;
    match result {
        Ok(status) if status.success() => {}
        Ok(status) => on_event(Event::Error(format!("systemctl --user start masterkeysd failed ({status})"))),
        Err(e) => on_event(Event::Error(format!("cannot run systemctl: {e}"))),
    }
}

/// One session with the daemon on `conn`; returns when the daemon goes away.
pub async fn run_on(
    conn: zbus::Connection,
    on_event: &(impl Fn(Event) + ?Sized),
    commands: &mut mpsc::UnboundedReceiver<Command>,
) {
    let mut held = HashSet::new();
    if let Err(e) = session(&conn, on_event, commands, &mut held).await {
        log_session_end(&e);
    }
    on_event(Event::Status(Status::DaemonNotRunning));
    for code in held {
        on_event(Event::Key(code, false));
    }
}

fn log_session_end(error: &zbus::Error) {
    eprintln!("masterkeys: daemon connection ended: {error}");
}

async fn session(
    conn: &zbus::Connection,
    on_event: &(impl Fn(Event) + ?Sized),
    commands: &mut mpsc::UnboundedReceiver<Command>,
    held: &mut HashSet<u16>,
) -> zbus::Result<()> {
    let proxy = DaemonProxy::new(conn).await?;
    let mut states = proxy.receive_state_changed().await?;
    let mut devices = proxy.receive_device_connected().await?;
    let mut keys = proxy.receive_key_event().await?;
    let mut profile_changes = proxy.receive_profiles_changed().await?;
    // On the session bus, notice the daemon leaving; peer-to-peer connections have no bus driver.
    let mut owner_changes = match conn.unique_name() {
        Some(_) => Some(zbus::fdo::DBusProxy::new(conn).await?.receive_name_owner_changed().await?),
        None => None,
    };

    let mut status = None;
    let report = |r: StateReport, status: &mut Option<Status>| {
        let now = if r.connected { Status::Connected } else { Status::KeyboardNotFound };
        if *status != Some(now) {
            *status = Some(now);
            on_event(Event::Status(now));
        }
        on_event(Event::State(Box::new(r)));
    };
    report(serde_json::from_str(&proxy.get_state().await?).map_err(json_error)?, &mut status);
    list_profiles(&proxy, on_event).await?;
    proxy.subscribe_keys().await?;

    loop {
        tokio::select! {
            signal = states.next() => {
                let Some(signal) = signal else { return Ok(()) };
                let json = signal.args()?.state_json;
                report(serde_json::from_str(&json).map_err(json_error)?, &mut status);
            }
            signal = profile_changes.next() => {
                if signal.is_none() { return Ok(()) }
                list_profiles(&proxy, on_event).await?;
            }
            signal = devices.next() => {
                let Some(signal) = signal else { return Ok(()) };
                let now = if signal.args()?.connected { Status::Connected } else { Status::KeyboardNotFound };
                if status != Some(now) {
                    status = Some(now);
                    on_event(Event::Status(now));
                }
                if now == Status::KeyboardNotFound {
                    for code in held.drain() {
                        on_event(Event::Key(code, false));
                    }
                }
            }
            signal = keys.next() => {
                let Some(signal) = signal else { return Ok(()) };
                let args = signal.args()?;
                if args.pressed { held.insert(args.code); } else { held.remove(&args.code); }
                on_event(Event::Key(args.code, args.pressed));
            }
            change = next_owner_change(&mut owner_changes) => {
                let Some(change) = change else { return Ok(()) };
                let args = change.args()?;
                if args.name().as_str() == BUS_NAME && args.new_owner().is_none() {
                    return Ok(());
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return Ok(()) };
                if let Err(e) = send(&proxy, command).await {
                    on_event(Event::Error(e.to_string()));
                }
            }
        }
    }
}

async fn next_owner_change(
    stream: &mut Option<zbus::fdo::NameOwnerChangedStream>,
) -> Option<zbus::fdo::NameOwnerChanged> {
    match stream {
        Some(stream) => stream.next().await,
        None => std::future::pending().await,
    }
}

async fn list_profiles(proxy: &DaemonProxy<'_>, on_event: &(impl Fn(Event) + ?Sized)) -> zbus::Result<()> {
    let list = serde_json::from_str(&proxy.list_profiles().await?).map_err(json_error)?;
    on_event(Event::Profiles(list));
    Ok(())
}

async fn send(proxy: &DaemonProxy<'_>, command: Command) -> zbus::Result<()> {
    match command {
        Command::SetAll(color) => proxy.set_all_color(&to_hex(color)).await,
        Command::SetZone(zone, color) => proxy.set_zone_color(zone, &to_hex(color)).await,
        Command::SetBrightness(percent) => proxy.set_brightness(percent).await,
        Command::SetEffect(effect) => proxy.set_effect(&serde_json::to_string(&effect).map_err(json_error)?).await,
        Command::ActivateProfile(name) => proxy.activate_profile(&name).await,
        Command::CreateProfile(name) => proxy.create_profile(&name).await,
        Command::DeleteProfile(name) => proxy.delete_profile(&name).await,
        Command::RenameProfile(old, new) => proxy.rename_profile(&old, &new).await,
        Command::SetExecutables(name, exes) => {
            let exes: Vec<&str> = exes.iter().map(String::as_str).collect();
            proxy.set_profile_executables(&name, &exes).await
        }
        Command::SetDefault(name) => proxy.set_default_profile(&name).await,
        Command::SetRemap(key, target) => proxy.set_remap(&key, &target).await,
        Command::ClearRemap(key) => proxy.clear_remap(&key).await,
        Command::SetMacro(key, m) => proxy.set_macro(&key, &serde_json::to_string(&m).map_err(json_error)?).await,
        Command::ClearMacro(key) => proxy.clear_macro(&key).await,
        Command::StartRecording => proxy.start_macro_recording().await,
        Command::StopRecording(key) => {
            let json = proxy.stop_macro_recording().await?;
            proxy.set_macro(&key, &json).await
        }
        Command::StartDaemon => Ok(()),
    }
}

fn json_error(e: serde_json::Error) -> zbus::Error {
    zbus::Error::Failure(format!("bad JSON from daemon: {e}"))
}
