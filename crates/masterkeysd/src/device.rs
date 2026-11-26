//! Lighting devices behind a trait, so the output loop can be tested with fakes.

use masterkeys_core::devices::DeviceInfo;
use steelseries_gg::apex3::{Apex3Device, ZONE_COUNT};
use steelseries_gg::rgb::Color;

/// What the output loop needs from a lighting device.
pub trait Lighting: Send {
    fn info(&self) -> &DeviceInfo;
    /// One colour per `info().leds`, same order.
    fn write(&mut self, colors: &[Color]) -> steelseries_gg::Result<()>;
    fn set_brightness(&mut self, percent: u8) -> steelseries_gg::Result<()>;
    /// Shortest time between two `write`s the device accepts; the output loop waits it out.
    fn min_gap(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }
}

/// The Apex 3 with its description.
struct Apex3 {
    device: Apex3Device,
    info: DeviceInfo,
}

impl Lighting for Apex3 {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn write(&mut self, colors: &[Color]) -> steelseries_gg::Result<()> {
        let zones: [Color; ZONE_COUNT] = std::array::from_fn(|i| colors.get(i).copied().unwrap_or(Color::BLACK));
        self.device.set_zones(&zones)
    }

    fn set_brightness(&mut self, percent: u8) -> steelseries_gg::Result<()> {
        self.device.set_brightness(percent)
    }
}

/// Opens a device; called again after every disconnect.
pub type Opener = Box<dyn FnMut() -> steelseries_gg::Result<Box<dyn Lighting>> + Send>;

/// Opens the real Apex 3.
pub fn apex3_opener() -> Opener {
    Box::new(|| Ok(Box::new(Apex3 { device: Apex3Device::open()?, info: DeviceInfo::apex3() }) as Box<dyn Lighting>))
}

/// Opens a real K745 PRO on its cable.
pub fn k745_opener() -> Opener {
    Box::new(|| Ok(Box::new(crate::redragon::K745::open()?) as Box<dyn Lighting>))
}
