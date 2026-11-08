//! The keyboard behind a trait, so the output loop can be tested with a fake.

use steelseries_gg::apex3::{Apex3Device, ZONE_COUNT};
use steelseries_gg::rgb::Color;

/// What the output loop needs from a keyboard.
pub trait Lighting: Send {
    fn set_zones(&mut self, colors: &[Color; ZONE_COUNT]) -> steelseries_gg::Result<()>;
    fn set_brightness(&mut self, percent: u8) -> steelseries_gg::Result<()>;
}

impl Lighting for Apex3Device {
    fn set_zones(&mut self, colors: &[Color; ZONE_COUNT]) -> steelseries_gg::Result<()> {
        Apex3Device::set_zones(self, colors)
    }

    fn set_brightness(&mut self, percent: u8) -> steelseries_gg::Result<()> {
        Apex3Device::set_brightness(self, percent)
    }
}

/// Opens a keyboard; called again after every disconnect.
pub type Opener = Box<dyn FnMut() -> steelseries_gg::Result<Box<dyn Lighting>> + Send>;

/// Opens the real Apex 3.
pub fn apex3_opener() -> Opener {
    Box::new(|| Ok(Box::new(Apex3Device::open()?) as Box<dyn Lighting>))
}
