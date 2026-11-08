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

#[derive(Debug)]
enum Command {
    All(Color),
    Zones([Color; ZONE_COUNT]),
    Brightness(u8),
}

fn parse(args: &[String]) -> Result<Command, String> {
    match args {
        [cmd, hex] if cmd == "all" => Ok(Command::All(color(hex)?)),
        [cmd, rest @ ..] if cmd == "zones" && rest.len() == ZONE_COUNT => {
            let mut zones = [Color::BLACK; ZONE_COUNT];
            for (zone, hex) in zones.iter_mut().zip(rest) {
                *zone = color(hex)?;
            }
            Ok(Command::Zones(zones))
        }
        [cmd, n] if cmd == "brightness" => match n.parse::<u8>() {
            Ok(level) if level <= 100 => Ok(Command::Brightness(level)),
            _ => Err(format!("brightness must be 0-100, got {n}")),
        },
        _ => Err(USAGE.into()),
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let command = parse(args)?;
    let mut device = Apex3Device::open().map_err(|e| e.to_string())?;
    match command {
        Command::All(color) => device.set_all(color),
        Command::Zones(zones) => device.set_zones(&zones),
        Command::Brightness(level) => device.set_brightness(level),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn typo_gives_usage_without_touching_the_device() {
        assert_eq!(parse(&args("al ff0000")).unwrap_err(), USAGE);
    }

    #[test]
    fn brightness_above_255_reports_the_range() {
        let err = parse(&args("brightness 300")).unwrap_err();
        assert!(err.contains("0-100"), "{err}");
    }

    #[test]
    fn zones_needs_exactly_ten_colours() {
        assert!(matches!(parse(&args("zones ff0000 00ff00")), Err(e) if e == USAGE));
    }

    #[test]
    fn valid_commands_parse() {
        assert!(matches!(parse(&args("all ff0000")), Ok(Command::All(Color::RED))));
        assert!(matches!(parse(&args("brightness 40")), Ok(Command::Brightness(40))));
    }
}
