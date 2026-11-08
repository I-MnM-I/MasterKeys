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

    #[zbus(signal)]
    fn state_changed(&self, state_json: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn device_connected(&self, connected: bool) -> zbus::Result<()>;
}
