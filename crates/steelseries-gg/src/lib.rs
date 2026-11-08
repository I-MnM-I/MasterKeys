//! # steelseries-gg (MasterKeys fork)
//!
//! Device library for the SteelSeries Apex 3 (1038:161a), trimmed from
//! steelseriesgg-rs. See UPSTREAM.md.

pub mod apex3;
pub mod config;
pub mod error;
pub mod fs_utils;
pub mod gamesense;
pub mod input;
pub mod performance;
pub mod profiles;
pub mod rgb;

pub use error::{Error, Result};

/// SteelSeries USB Vendor ID (official USB-IF assignment)
/// Used to filter HID devices during enumeration
pub const STEELSERIES_VENDOR_ID: u16 = 0x1038;

