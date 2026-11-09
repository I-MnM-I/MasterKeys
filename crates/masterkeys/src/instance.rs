//! One GUI at a time: a second launch raises the running window and exits.

use std::sync::{Arc, Mutex};
use zbus::fdo::{RequestNameFlags, RequestNameReply};

const NAME: &str = "org.masterkeys.Gui";
const PATH: &str = "/org/masterkeys/Gui";

type ShowFn = Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>;

struct GuiService {
    show: ShowFn,
}

#[zbus::interface(name = "org.masterkeys.Gui1")]
impl GuiService {
    /// Show the window (from a second launch).
    fn show(&self) {
        if let Some(show) = &*self.show.lock().unwrap() {
            show();
        }
    }
}

/// This process's claim on being the GUI; its bus connection lives on a thread of its own.
pub struct GuiInstance {
    show: ShowFn,
}

impl GuiInstance {
    /// What a second launch's `Show` call does.
    pub fn on_show(&self, show: impl Fn() + Send + Sync + 'static) {
        *self.show.lock().unwrap() = Some(Box::new(show));
    }
}

/// Become the GUI, or raise the one already running and return `None`. Without a session bus the
/// GUI just runs.
pub fn claim_or_raise() -> Option<GuiInstance> {
    let show: ShowFn = Arc::default();
    let (tx, rx) = std::sync::mpsc::channel();
    let service_show = show.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
        runtime.block_on(async move {
            let claimed = claim(service_show).await;
            let keep = claimed.as_ref().ok().cloned();
            let _ = tx.send(claimed.map(|_| ()));
            if keep.is_some() {
                std::future::pending::<()>().await; // keep serving Show
            }
        });
    });
    match rx.recv() {
        Ok(Ok(())) => Some(GuiInstance { show }),
        Ok(Err(Claim::Taken)) => None,
        Ok(Err(Claim::NoBus(e))) => {
            eprintln!("masterkeys: no session bus ({e}); not checking for another window");
            Some(GuiInstance { show })
        }
        Err(_) => Some(GuiInstance { show }),
    }
}

enum Claim {
    /// Another GUI owns the name and was asked to show itself.
    Taken,
    /// No usable session bus; run anyway.
    NoBus(String),
}

async fn claim(show: ShowFn) -> Result<zbus::Connection, Claim> {
    let conn = zbus::Connection::session().await.map_err(|e| Claim::NoBus(e.to_string()))?;
    if let Err(e) = conn.object_server().at(PATH, GuiService { show }).await {
        eprintln!("masterkeys: cannot serve {NAME}: {e}");
    }
    match conn.request_name_with_flags(NAME, RequestNameFlags::DO_NOT_QUEUE).await {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => Ok(conn),
        Ok(_) | Err(zbus::Error::NameTaken) => {
            if let Err(e) = conn.call_method(Some(NAME), PATH, Some("org.masterkeys.Gui1"), "Show", &()).await {
                eprintln!("masterkeys: another window is running but did not answer: {e}");
            }
            Err(Claim::Taken)
        }
        Err(e) => {
            eprintln!("masterkeys: cannot claim {NAME}: {e}");
            Ok(conn)
        }
    }
}
