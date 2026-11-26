//! Windows input: one low-level keyboard hook for the life of the process, and SendInput. While
//! grabbing, the hook keeps physical keys from Windows and the engine replays them, as the evdev grab
//! does on Linux; `hookrules` decides each event.

use super::{EV_KEY, Ended, GrabDecision, KeySink, RawEvent, grab_decision, pump};
use crate::engine::Engine;
use crate::hookrules::{Action, HookMode, HookState, MASTERKEYS_TAG, is_injected, should_beat};
use crate::keys::PressTracker;
use crate::winkeys::{Out, hook_code, send_as};
use masterkeys_core::bindings::Compiled;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use steelseries_gg::input::{KeyEvent, KeyValue};
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, SetWindowsHookExW,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
};

const HEARTBEAT: Duration = Duration::from_millis(100);

struct Hook {
    mode: AtomicU8,
    /// Milliseconds since `start` when the core last showed it is alive.
    heartbeat: AtomicU64,
    start: Instant,
    installed: AtomicBool,
    state: Mutex<HookState>,
    events: Mutex<Option<mpsc::UnboundedSender<RawEvent>>>,
}

static HOOK: OnceLock<Hook> = OnceLock::new();

fn hook() -> &'static Hook {
    HOOK.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("masterkeys-hook".into())
            .spawn(move || run_hook_thread(tx))
            .expect("hook thread");
        let installed = rx.recv().unwrap_or(false);
        if !installed {
            warn!("cannot install the keyboard hook");
        }
        Hook {
            mode: AtomicU8::new(HookMode::Off as u8),
            heartbeat: AtomicU64::new(0),
            start: Instant::now(),
            installed: AtomicBool::new(installed),
            state: Mutex::new(HookState::default()),
            events: Mutex::new(None),
        }
    })
}

fn run_hook_thread(installed: std::sync::mpsc::Sender<bool>) {
    // SAFETY: plain Win32 calls; the hook procedure is a static function and the message loop keeps it
    // running on this thread.
    unsafe {
        let handle = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), GetModuleHandleW(std::ptr::null()), 0);
        let _ = installed.send(!handle.is_null());
        if handle.is_null() {
            return;
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {}
    }
}

fn mode_of(raw: u8) -> HookMode {
    match raw {
        x if x == HookMode::Read as u8 => HookMode::Read,
        x if x == HookMode::Grab as u8 => HookMode::Grab,
        _ => HookMode::Off,
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lparam points at the event's KBDLLHOOKSTRUCT.
        let k = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        if let (Some(h), Some(key)) =
            (HOOK.get(), hook_code(k.vkCode as u16, k.scanCode as u16, k.flags & LLKHF_EXTENDED != 0))
        {
            let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let ours = is_injected(k.flags, k.dwExtraInfo);
            let now = h.start.elapsed().as_millis() as u64;
            let age = Duration::from_millis(now.saturating_sub(h.heartbeat.load(Ordering::Relaxed)));
            let mode = mode_of(h.mode.load(Ordering::Relaxed));
            let mut state = h.state.lock().unwrap();
            let (action, value) = state.on_key(mode, key, down, ours, age);
            match action {
                Action::Pass => {}
                Action::Drop => return 1,
                Action::Report | Action::Swallow => {
                    let grabbed = action == Action::Swallow;
                    let sent = h
                        .events
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|tx| tx.send(RawEvent { grabbed, kind: EV_KEY, code: key, value }).is_ok());
                    if grabbed && sent {
                        return 1;
                    }
                    if grabbed {
                        state.not_delivered(key);
                    }
                }
            }
        }
    }
    // SAFETY: passing the event on, as every hook procedure must.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Point the hook's events at a new channel and switch its mode.
fn listen(mode: HookMode) -> mpsc::UnboundedReceiver<RawEvent> {
    let h = hook();
    let (tx, rx) = mpsc::unbounded_channel();
    *h.events.lock().unwrap() = Some(tx);
    h.mode.store(mode as u8, Ordering::Relaxed);
    rx
}

fn stop_listening() {
    let h = hook();
    h.mode.store(HookMode::Off as u8, Ordering::Relaxed);
    *h.events.lock().unwrap() = None;
}

fn beat() {
    let h = hook();
    h.heartbeat.store(h.start.elapsed().as_millis() as u64, Ordering::Relaxed);
}

/// Report key transitions to the GUI until `stop` turns true. Always ends by releasing held keys.
pub async fn read_keys(events: mpsc::Sender<(u16, bool)>, mut stop: watch::Receiver<bool>) {
    let mut raw = listen(HookMode::Read);
    let mut tracker = PressTracker::new();
    loop {
        tokio::select! {
            event = raw.recv() => match event {
                Some(e) => if let Some(t) = tracker.feed(e.code, e.value) { let _ = events.send(t).await; },
                None => break,
            },
            _ = async { let _ = stop.wait_for(|stop| *stop).await; } => break,
        }
    }
    stop_listening();
    for t in tracker.release_all() {
        let _ = events.send(t).await;
    }
}

/// Virtual keys held down right now (mouse buttons left out).
fn held_keys() -> Vec<u16> {
    // SAFETY: GetAsyncKeyState only reads key state.
    (0x08u16..=0xFE).filter(|&vk| unsafe { GetAsyncKeyState(i32::from(vk)) } as u16 & 0x8000 != 0).collect()
}

/// Take the keyboard and run it through the engine until `pump` ends.
pub async fn grab_session(bindings: watch::Receiver<Compiled>, gui: &mpsc::Sender<(u16, bool)>) -> Ended {
    if !hook().installed.load(Ordering::Relaxed) {
        return Ended::Error("cannot install the keyboard hook".into());
    }
    // Wait for held keys to come up, so Windows never misses their release.
    let waited = tokio::time::Instant::now();
    loop {
        match grab_decision(&[held_keys()], waited.elapsed()) {
            GrabDecision::Grab => break,
            GrabDecision::Wait => tokio::time::sleep(Duration::from_millis(50)).await,
            GrabDecision::GiveUp => return Ended::KeysHeld,
        }
    }
    beat();
    let (raw_tx, raw_rx) = mpsc::channel(256);
    // Beat only while the engine has drained what it was given (see `should_beat`).
    let queue = raw_tx.clone();
    let heartbeat = tokio::spawn(async move {
        loop {
            if should_beat(queue.max_capacity() - queue.capacity()) {
                beat();
            }
            tokio::time::sleep(HEARTBEAT).await;
        }
    });
    let mut hook_rx = listen(HookMode::Grab);
    let forward = tokio::spawn(async move {
        while let Some(e) = hook_rx.recv().await {
            if raw_tx.send(e).await.is_err() {
                break;
            }
        }
    });
    info!("keyboard hooked, remapping");
    let mut engine = Engine::new(&bindings.borrow());
    let ended = pump(raw_rx, &mut engine, bindings, &mut SendInputSink, gui).await;
    stop_listening();
    heartbeat.abort();
    forward.abort();
    info!("keyboard released ({ended:?})");
    ended
}

/// Sends the engine's output as injected key events.
struct SendInputSink;

impl KeySink for SendInputSink {
    fn emit(&mut self, events: &[KeyEvent]) -> io::Result<()> {
        let inputs: Vec<INPUT> = events.iter().filter_map(input_for).collect();
        if inputs.is_empty() {
            return Ok(());
        }
        // SAFETY: `inputs` is a valid array of keyboard INPUTs.
        let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
        if (sent as usize) < inputs.len() {
            // Windows refuses input to programs running as administrator; keep remapping for the rest.
            warn!("Windows took {sent} of {} key events: {}", inputs.len(), io::Error::last_os_error());
        }
        Ok(())
    }
}

fn input_for(e: &KeyEvent) -> Option<INPUT> {
    let up = if e.value == KeyValue::Release { KEYEVENTF_KEYUP } else { 0 };
    let ki = match send_as(e.key.code())? {
        Out::Scan(scan, extended) => KEYBDINPUT {
            wVk: 0,
            wScan: scan,
            dwFlags: KEYEVENTF_SCANCODE | up | if extended { KEYEVENTF_EXTENDEDKEY } else { 0 },
            time: 0,
            dwExtraInfo: MASTERKEYS_TAG,
        },
        Out::Vk(vk) => KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: up, time: 0, dwExtraInfo: MASTERKEYS_TAG },
    };
    Some(INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } })
}
