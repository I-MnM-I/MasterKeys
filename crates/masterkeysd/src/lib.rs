//! masterkeysd internals, as a library so integration tests can reach them.

pub mod app;
#[cfg(target_os = "linux")]
pub mod coreprops;
pub mod device;
pub mod embedded;
pub mod engine;
pub mod gamesense;
pub mod input;
pub mod keys;
pub mod output;
pub mod p2p;
pub mod persist;
pub mod profiles;
pub mod service;
