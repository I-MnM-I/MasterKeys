//! D-Bus names and client proxy for masterkeysd.
//!
//! Colours travel as `RRGGBB` strings and states/effects as JSON, so `busctl` can drive everything.

use zbus::proxy;

pub const BUS_NAME: &str = "org.masterkeys.Daemon";
pub const OBJECT_PATH: &str = "/org/masterkeys/Daemon";
pub const INTERFACE: &str = "org.masterkeys.Daemon1";

#[proxy(
    interface = "org.masterkeys.Daemon1",
    default_service = "org.masterkeys.Daemon",
    default_path = "/org/masterkeys/Daemon"
)]
pub trait Daemon {
    /// JSON `StateReport`.
    async fn get_state(&self) -> zbus::Result<String>;
    async fn set_all_color(&self, color: &str) -> zbus::Result<()>;
    async fn set_zone_color(&self, zone: u8, color: &str) -> zbus::Result<()>;
    async fn set_brightness(&self, percent: u8) -> zbus::Result<()>;
    /// JSON `Effect`.
    async fn set_effect(&self, effect_json: &str) -> zbus::Result<()>;

    /// JSON list of `ProfileSummary`, sorted by name.
    async fn list_profiles(&self) -> zbus::Result<String>;
    async fn activate_profile(&self, name: &str) -> zbus::Result<()>;
    /// A new profile copying the current lighting; not activated.
    async fn create_profile(&self, name: &str) -> zbus::Result<()>;
    async fn delete_profile(&self, name: &str) -> zbus::Result<()>;
    async fn rename_profile(&self, old: &str, new: &str) -> zbus::Result<()>;
    async fn set_profile_executables(&self, name: &str, executables: &[&str]) -> zbus::Result<()>;
    async fn set_default_profile(&self, name: &str) -> zbus::Result<()>;

    /// Start receiving `KeyEvent` (sent only to subscribers, only while subscribed).
    /// Remap a key on the active profile, e.g. `("caps_lock", "esc")` or `("f13", "left_ctrl+c")`.
    async fn set_remap(&self, key: &str, target: &str) -> zbus::Result<()>;
    async fn clear_remap(&self, key: &str) -> zbus::Result<()>;
    /// JSON `Macro` on a key of the active profile.
    async fn set_macro(&self, key: &str, macro_json: &str) -> zbus::Result<()>;
    async fn clear_macro(&self, key: &str) -> zbus::Result<()>;
    async fn start_macro_recording(&self) -> zbus::Result<()>;
    /// JSON `Macro` of the keys pressed since `StartMacroRecording`.
    async fn stop_macro_recording(&self) -> zbus::Result<String>;
    async fn subscribe_keys(&self) -> zbus::Result<()>;
    async fn unsubscribe_keys(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn state_changed(&self, state_json: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn device_connected(&self, connected: bool) -> zbus::Result<()>;
    #[zbus(signal)]
    fn profiles_changed(&self) -> zbus::Result<()>;
    /// `code` is the Linux evdev key code.
    #[zbus(signal)]
    fn key_event(&self, code: u16, pressed: bool) -> zbus::Result<()>;
}
