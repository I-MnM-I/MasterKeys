//! Drive the Apex 3 lighting from the command line.
//!
//! apex3_smoke all RRGGBB | zones RRGGBB x10 | brightness N

use std::process::ExitCode;
use steelseries_gg::apex3::{Apex3Device, ZONE_COUNT};
use steelseries_gg::rgb::Color;

const USAGE: &str = "usage: apex3_smoke all RRGGBB | zones RRGGBB x10 | brightness 0-100";

fn color(arg: &str) -> Result<Color, String> {
    Color::parse(arg).ok_or_else(|| format!("not a colour: {arg}"))
}

fn run(args: &[String]) -> Result<(), String> {
    let mut device = Apex3Device::open().map_err(|e| e.to_string())?;
    match args {
        [cmd, hex] if cmd == "all" => device.set_all(color(hex)?),
        [cmd, rest @ ..] if cmd == "zones" && rest.len() == ZONE_COUNT => {
            let mut zones = [Color::BLACK; ZONE_COUNT];
            for (zone, hex) in zones.iter_mut().zip(rest) {
                *zone = color(hex)?;
            }
            device.set_zones(&zones)
        }
        [cmd, n] if cmd == "brightness" => {
            device.set_brightness(n.parse().map_err(|_| format!("not a number: {n}"))?)
        }
        _ => return Err(USAGE.into()),
    }
    .map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("apex3_smoke: {e}");
            ExitCode::FAILURE
        }
    }
}
