//! SteelSeries Apex 3 (1038:161a) lighting I/O.
//!
//! Interface 3 (vendor usage page 0xFFC0) takes 32-byte output reports with no report ID, so every write is
//! 33 bytes: `0x00` + payload. Frames verified on hardware (`docs/apex3-protocol.md`):
//! colours `00 0B 00 <R G B x10>`, brightness `00 0A 00 <0..=100>`. Never sends `00 09 00` (save_to_flash).

use crate::rgb::Color;
use crate::{Error, Result, STEELSERIES_VENDOR_ID};
use hidapi::{HidApi, HidDevice};

/// Lighting zones: vertical bands, index 0 leftmost through 9 rightmost.
pub const ZONE_COUNT: usize = 10;
/// Bytes per hidraw write: report ID 0 + 32-byte payload.
pub const REPORT_LEN: usize = 33;

const PRODUCT_ID: u16 = 0x161A;
const CONTROL_INTERFACE: i32 = 3;
const CMD_COLORS: u8 = 0x0B;
const CMD_BRIGHTNESS: u8 = 0x0A;

/// The frame that sets all ten zone colours.
pub fn colors_report(colors: &[Color; ZONE_COUNT]) -> [u8; REPORT_LEN] {
    let mut report = [0u8; REPORT_LEN];
    report[1] = CMD_COLORS;
    for (slot, color) in report[3..].chunks_exact_mut(3).zip(colors) {
        slot.copy_from_slice(&[color.r, color.g, color.b]);
    }
    report
}

/// The frame that sets global brightness, 0-100.
pub fn brightness_report(percent: u8) -> Result<[u8; REPORT_LEN]> {
    if percent > 100 {
        return Err(Error::InvalidConfig(format!("Apex 3 brightness {percent} out of range 0-100")));
    }
    let mut report = [0u8; REPORT_LEN];
    report[1] = CMD_BRIGHTNESS;
    report[3] = percent;
    Ok(report)
}

/// Whether a HID interface is the Apex 3's lighting control interface (never the typing or media ones).
pub fn is_apex3_control(vendor: u16, product: u16, interface: i32) -> bool {
    vendor == STEELSERIES_VENDOR_ID && product == PRODUCT_ID && interface == CONTROL_INTERFACE
}

fn not_found() -> Error {
    Error::DeviceNotFound("SteelSeries Apex 3 (1038:161a) is not connected".into())
}

fn open_error(message: String) -> Error {
    if message.contains("Permission denied") {
        Error::PermissionDenied(format!(
            "cannot open the Apex 3 ({message}); install assets/70-masterkeys.rules into /etc/udev/rules.d/"
        ))
    } else {
        Error::DeviceCommunication(message)
    }
}

/// An open handle on the Apex 3's lighting interface.
pub struct Apex3Device {
    device: HidDevice,
}

impl Apex3Device {
    /// Find and open the connected Apex 3.
    pub fn open() -> Result<Self> {
        let api = HidApi::new()?;
        let info = api
            .device_list()
            .find(|d| is_apex3_control(d.vendor_id(), d.product_id(), d.interface_number()))
            .ok_or_else(not_found)?;
        let device = info.open_device(&api).map_err(|e| open_error(e.to_string()))?;
        Ok(Self { device })
    }

    /// Set every zone, index 0 leftmost.
    pub fn set_zones(&mut self, colors: &[Color; ZONE_COUNT]) -> Result<()> {
        self.write(&colors_report(colors))
    }

    /// Set all zones to one colour.
    pub fn set_all(&mut self, color: Color) -> Result<()> {
        self.set_zones(&[color; ZONE_COUNT])
    }

    /// Set global brightness, 0-100.
    pub fn set_brightness(&mut self, percent: u8) -> Result<()> {
        self.write(&brightness_report(percent)?)
    }

    fn write(&mut self, report: &[u8; REPORT_LEN]) -> Result<()> {
        let written = self.device.write(report)?;
        if written != REPORT_LEN {
            return Err(Error::DeviceCommunication(format!("short write: {written} of {REPORT_LEN} bytes")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use crate::rgb::Color;

    #[test]
    fn colors_report_layout() {
        let mut z = [Color::BLACK; ZONE_COUNT];
        z[0] = Color::RED;
        z[9] = Color::BLUE;
        let r = colors_report(&z);
        assert_eq!(r.len(), 33);
        assert_eq!(&r[..3], &[0x00, 0x0B, 0x00]);
        assert_eq!(&r[3..6], &[0xFF, 0x00, 0x00]);
        assert_eq!(&r[30..33], &[0x00, 0x00, 0xFF]);
    }

    #[test]
    fn brightness_report_layout() {
        let r = brightness_report(55).unwrap();
        assert_eq!(&r[..4], &[0x00, 0x0A, 0x00, 55]);
        assert!(r[4..].iter().all(|&b| b == 0));
    }

    #[test]
    fn brightness_over_100_rejected() {
        assert!(brightness_report(101).is_err());
    }

    #[test]
    fn only_interface_3_of_apex3_selected() {
        assert!(is_apex3_control(0x1038, 0x161A, 3));
        for i in 0..3 {
            assert!(!is_apex3_control(0x1038, 0x161A, i));
        }
        assert!(!is_apex3_control(0x1038, 0x1622, 3));
        assert!(!is_apex3_control(0x046D, 0x161A, 3));
    }

    #[test]
    fn no_report_starts_with_save_to_flash() {
        let r = colors_report(&[Color::BLACK; ZONE_COUNT]);
        assert_ne!(&r[..2], &[0x00, 0x09]);
        let r = brightness_report(0).unwrap();
        assert_ne!(&r[..2], &[0x00, 0x09]);
    }

    #[test]
    fn not_found_error_names_the_device() {
        let Error::DeviceNotFound(msg) = not_found() else { panic!("wrong variant") };
        assert!(msg.contains("1038:161a"), "{msg}");
    }

    #[test]
    fn permission_denied_open_error_points_at_udev_rule() {
        let Error::PermissionDenied(msg) = open_error("Failed to open a device with path '/dev/hidraw4': Permission denied".into()) else {
            panic!("wrong variant")
        };
        assert!(msg.contains("70-masterkeys.rules"), "{msg}");
    }

    #[test]
    fn other_open_errors_stay_communication_errors() {
        assert!(matches!(open_error("Device busy".into()), Error::DeviceCommunication(_)));
    }
}
