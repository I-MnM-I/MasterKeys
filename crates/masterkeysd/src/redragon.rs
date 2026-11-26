//! Redragon K745 PRO on its USB cable (SinoWealth / BY Tech firmware, 258a:010c).
//!
//! Only the volatile lighting frame is ever sent: feature report 0x06, command 0x06, 520 bytes, with
//! the red, green and blue planes for up to 126 LEDs. The frame format is the one published for the
//! MCHOSE G75 (same chip and USB ID). No save, flash or remap command exists in this file, and none may
//! be added: other boards share this USB ID, and wrong commands have bricked some of them.

use crate::device::Lighting;
use masterkeys_core::devices::{DeviceInfo, Led};
use std::time::{Duration, Instant};
use steelseries_gg::rgb::Color;

pub const K745_VID: u16 = 0x258a;
pub const K745_PID: u16 = 0x010c;
pub const FRAME_LEN: usize = 520;
pub const MIN_GAP: Duration = Duration::from_millis(35);
const INTERFACE: i32 = 1;
const USAGE_PAGE: u16 = 0xFF00;
const REPORT_ID: u8 = 0x06;
const PLANE: usize = 126;
const RED: usize = 8;
const GREEN: usize = RED + PLANE;
const BLUE: usize = GREEN + PLANE;

/// Whether a HID interface is the K745 PRO's wired lighting interface.
pub fn matches(vid: u16, pid: u16, interface: i32, usage_page: u16) -> bool {
    vid == K745_VID && pid == K745_PID && interface == INTERFACE && usage_page == USAGE_PAGE
}

/// Whether a report descriptor declares the 520-byte lighting feature report (ID 6, 519 payload bytes).
pub fn has_lighting_report(descriptor: &[u8]) -> bool {
    let (mut size, mut count, mut id) = (0u32, 0u32, 0u32);
    let mut stack = Vec::new();
    let mut bits = 0u32;
    let mut i = 0;
    while i < descriptor.len() {
        let prefix = descriptor[i];
        if prefix == 0xFE {
            // Long item: data size in the next byte, then the long tag.
            let len = descriptor.get(i + 1).copied().unwrap_or(0) as usize;
            i += 3 + len;
            continue;
        }
        let len = match prefix & 3 {
            3 => 4,
            n => n as usize,
        };
        let Some(data) = descriptor.get(i + 1..i + 1 + len) else { break };
        let value = data.iter().rev().fold(0u32, |v, &b| (v << 8) | u32::from(b));
        match prefix & 0xFC {
            0x74 => size = value,
            0x94 => count = value,
            0x84 => id = value,
            0xA4 => stack.push((size, count, id)),
            0xB4 => {
                if let Some(saved) = stack.pop() {
                    (size, count, id) = saved;
                }
            }
            0xB0 if id == u32::from(REPORT_ID) => bits += size * count,
            _ => {}
        }
        i += 1 + len;
    }
    bits == (FRAME_LEN as u32 - 1) * 8
}

/// The lighting frame for `colors` (one per `leds`), scaled by `brightness` (0-100).
pub fn encode_frame(leds: &[Led], colors: &[Color], brightness: u8) -> [u8; FRAME_LEN] {
    let mut f = [0u8; FRAME_LEN];
    f[..8].copy_from_slice(&[REPORT_ID, 0x06, 0, 0, 0x01, 0, 0x80, 0x01]);
    let scale = |c: u8| (u32::from(c) * u32::from(brightness.min(100)) / 100) as u8;
    for (led, color) in leds.iter().zip(colors) {
        let slot = usize::from(led.index);
        if slot < PLANE {
            f[RED + slot] = scale(color.r);
            f[GREEN + slot] = scale(color.g);
            f[BLUE + slot] = scale(color.b);
        }
    }
    f
}

/// An open K745 PRO lighting interface.
pub struct K745 {
    device: hidapi::HidDevice,
    info: DeviceInfo,
    brightness: u8,
    last_sent: Option<Instant>,
}

impl K745 {
    /// Find and open a K745 PRO on its cable; refuses anything whose descriptor lacks the lighting report.
    pub fn open() -> steelseries_gg::Result<K745> {
        let api = hidapi::HidApi::new()?;
        for d in api.device_list() {
            if !matches(d.vendor_id(), d.product_id(), d.interface_number(), d.usage_page()) {
                continue;
            }
            let Ok(device) = d.open_device(&api) else { continue };
            let mut descriptor = vec![0u8; 4096];
            let Ok(len) = device.get_report_descriptor(&mut descriptor) else { continue };
            if has_lighting_report(&descriptor[..len]) {
                return Ok(K745 { device, info: DeviceInfo::k745pro(), brightness: 100, last_sent: None });
            }
        }
        Err(steelseries_gg::Error::DeviceNotFound("no Redragon K745 PRO on a USB cable (258a:010c)".into()))
    }
}

impl Lighting for K745 {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn write(&mut self, colors: &[Color]) -> steelseries_gg::Result<()> {
        // The output loop already waits `min_gap`; this only guards against a caller that doesn't.
        if let Some(wait) = self.last_sent.and_then(|t| MIN_GAP.checked_sub(t.elapsed())) {
            std::thread::sleep(wait);
        }
        let frame = encode_frame(&self.info.leds, colors, self.brightness);
        self.device.send_feature_report(&frame)?;
        self.last_sent = Some(Instant::now());
        Ok(())
    }

    fn set_brightness(&mut self, percent: u8) -> steelseries_gg::Result<()> {
        self.brightness = percent.min(100);
        Ok(())
    }

    fn min_gap(&self) -> Duration {
        MIN_GAP
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leds() -> Vec<Led> {
        DeviceInfo::k745pro().leds
    }

    fn colors_with(set: &[(u16, Color)]) -> Vec<Color> {
        leds().iter().map(|l| set.iter().find(|(i, _)| *i == l.index).map_or(Color::BLACK, |(_, c)| *c)).collect()
    }

    fn header_ok(f: &[u8; FRAME_LEN]) {
        assert_eq!(&f[..8], &[0x06, 0x06, 0, 0, 0x01, 0, 0x80, 0x01]);
    }

    #[test]
    fn frame_bytes_for_known_colours() {
        let f = encode_frame(&leds(), &colors_with(&[(0, Color::RED), (5, Color::new(1, 2, 3))]), 100);
        header_ok(&f);
        assert_eq!((f[8], f[134], f[260]), (255, 0, 0));
        assert_eq!((f[13], f[139], f[265]), (1, 2, 3));
        let lit = [0usize, 1, 2, 3, 4, 5, 6, 7, 8, 13, 139, 265];
        for (i, b) in f.iter().enumerate() {
            if !lit.contains(&i) {
                assert_eq!(*b, 0, "byte {i}");
            }
        }
    }

    #[test]
    fn k745_brightness_scales_and_zero_is_black() {
        let colors = colors_with(&[(0, Color::new(255, 255, 255))]);
        let half = encode_frame(&leds(), &colors, 50);
        assert_eq!((half[8], half[134], half[260]), (127, 127, 127));
        let off = encode_frame(&leds(), &colors, 0);
        header_ok(&off);
        assert!(off[8..].iter().all(|&b| b == 0));
    }

    #[test]
    fn matcher_rejects_wireless_and_wrong_interface() {
        assert!(matches(0x258a, 0x010c, 1, 0xFF00));
        assert!(!matches(0x3554, 0xfa09, 1, 0xFF00));
        assert!(!matches(0x258a, 0x010c, 0, 0xFF00));
        assert!(!matches(0x258a, 0x010c, 1, 0x0001));
        assert!(!matches(0x258a, 0x0049, 1, 0xFF00));
    }

    /// Vendor page 0xFF00 application collection holding one feature report.
    fn descriptor(id: u8, count: u16) -> Vec<u8> {
        let mut d = vec![0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x85, id, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08];
        d.extend([0x96, count as u8, (count >> 8) as u8, 0x09, 0x01, 0xB1, 0x02, 0xC0]);
        d
    }

    #[test]
    fn descriptor_check() {
        assert!(has_lighting_report(&descriptor(6, 519)));
        assert!(!has_lighting_report(&descriptor(6, 63)));
        assert!(!has_lighting_report(&descriptor(5, 519)));
        assert!(!has_lighting_report(&[]));
    }
}
