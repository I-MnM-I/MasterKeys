//! Types shared by the MasterKeys daemon and GUI.

pub mod dbus;
pub mod state;

pub use state::{LightingState, StateReport, parse_hex, to_hex};
