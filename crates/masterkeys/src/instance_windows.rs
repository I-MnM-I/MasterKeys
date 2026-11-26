//! One GUI at a time on Windows: a named mutex, and a named event a second launch sets to raise the
//! first.

use super::{GuiInstance, ShowFn};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent, WaitForSingleObject,
};
use windows_sys::w;

/// Become the GUI, or raise the one already running and return `None`.
pub fn claim_or_raise() -> Option<GuiInstance> {
    let show: ShowFn = Default::default();
    // SAFETY: Win32 handle calls with checked results. The mutex stays open for the life of the process
    // (that is the claim); the event is waited on by its own thread.
    unsafe {
        let mutex = CreateMutexW(std::ptr::null(), 0, w!("Local\\MasterKeys"));
        if mutex.is_null() {
            return Some(GuiInstance { show });
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(mutex);
            let event = OpenEventW(EVENT_MODIFY_STATE, 0, w!("Local\\MasterKeys.Show"));
            if !event.is_null() {
                SetEvent(event);
                CloseHandle(event);
            }
            return None;
        }
        let event = CreateEventW(std::ptr::null(), 0, 0, w!("Local\\MasterKeys.Show"));
        if !event.is_null() {
            let event = event as usize; // handles are process-wide; pass it to the thread as a number
            let shown = show.clone();
            std::thread::spawn(move || {
                while WaitForSingleObject(event as _, INFINITE) == 0 {
                    if let Some(show) = &*shown.lock().unwrap() {
                        show();
                    }
                }
            });
        }
    }
    Some(GuiInstance { show })
}
