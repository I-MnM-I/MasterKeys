//! Windows input: a low-level keyboard hook and SendInput.

use super::Ended;
use masterkeys_core::bindings::Compiled;
use tokio::sync::{mpsc, watch};

/// Report key transitions to the GUI until `stop` turns true.
pub async fn read_keys(_events: mpsc::Sender<(u16, bool)>, mut stop: watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// Take over the keyboard and run it through the engine until the pump ends.
pub async fn grab_session(_bindings: watch::Receiver<Compiled>, _gui: &mpsc::Sender<(u16, bool)>) -> Ended {
    Ended::Error("remapping on Windows is not built yet".into())
}
