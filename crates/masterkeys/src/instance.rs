//! One GUI at a time: a second launch raises the running window and exits.

use std::sync::{Arc, Mutex};

type ShowFn = Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>;

#[cfg(not(windows))]
#[path = "instance_linux.rs"]
mod imp;
#[cfg(windows)]
#[path = "instance_windows.rs"]
mod imp;
pub use imp::claim_or_raise;

/// This process's claim on being the GUI; its bus connection lives on a thread of its own.
pub struct GuiInstance {
    pub(crate) show: ShowFn,
}

impl GuiInstance {
    /// What a second launch's `Show` call does.
    pub fn on_show(&self, show: impl Fn() + Send + Sync + 'static) {
        *self.show.lock().unwrap() = Some(Box::new(show));
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn second_claim_raises_the_first() {
        let first = claim_or_raise().expect("first claim");
        let raised = Arc::new(AtomicBool::new(false));
        let flag = raised.clone();
        first.on_show(move || flag.store(true, Ordering::SeqCst));
        assert!(claim_or_raise().is_none());
        let deadline = Instant::now() + Duration::from_secs(1);
        while !raised.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(raised.load(Ordering::SeqCst));
    }
}
