//! Macros as editable text, one step per line: `down KEY`, `up KEY`, `tap KEY`, `wait MS`.

use crate::bindings::{MAX_DELAY_MS, Macro, MacroStep, canonical_key, validate_macro};

/// The steps as text; a press followed straight away by its release becomes one `tap` line.
pub fn to_text(steps: &[MacroStep]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < steps.len() {
        let line = match (&steps[i], steps.get(i + 1)) {
            (MacroStep::Down(a), Some(MacroStep::Up(b))) if a == b => {
                i += 1;
                format!("tap {a}")
            }
            (MacroStep::Down(k), _) => format!("down {k}"),
            (MacroStep::Up(k), _) => format!("up {k}"),
            (MacroStep::Delay(ms), _) => format!("wait {ms}"),
        };
        out.push_str(&line);
        out.push('\n');
        i += 1;
    }
    out
}

/// Steps from text (blank lines ignored, case-insensitive); the error names the line at fault.
pub fn parse_text(text: &str) -> Result<Vec<MacroStep>, String> {
    let mut steps = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let mut words = line.split_whitespace();
        let Some(verb) = words.next() else { continue };
        let verb = verb.to_ascii_lowercase();
        let arg = words.next();
        let at = |e: String| format!("line {}: {e}", n + 1);
        let key = |what: &str| -> Result<String, String> {
            let name = arg.ok_or_else(|| at(format!("{what} needs a key")))?;
            canonical_key(name).map_err(|_| at(format!("unknown key {:?}", name.to_ascii_lowercase())))
        };
        match verb.as_str() {
            "down" => steps.push(MacroStep::Down(key("down")?)),
            "up" => steps.push(MacroStep::Up(key("up")?)),
            "tap" => {
                let k = key("tap")?;
                steps.push(MacroStep::Down(k.clone()));
                steps.push(MacroStep::Up(k));
            }
            "wait" => {
                let ms: u32 = arg
                    .and_then(|a| a.parse().ok())
                    .ok_or_else(|| at("wait needs a number of milliseconds".into()))?;
                if ms > MAX_DELAY_MS {
                    return Err(at(format!("a wait can be at most {MAX_DELAY_MS} ms")));
                }
                steps.push(MacroStep::Delay(ms));
            }
            other => return Err(at(format!("unknown step {other:?} (use down, up, tap or wait)"))),
        }
    }
    if steps.is_empty() {
        return Err("the macro is empty".into());
    }
    let check = Macro { steps, ..Macro::default() };
    if let Err(e) = validate_macro(&check) {
        return Err(match e.strip_prefix("macro never releases ") {
            Some(key) => format!("{key} is pressed but never released"),
            None => e,
        });
    }
    Ok(check.steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::MacroStep::{Delay, Down, Up};

    fn s(x: &str) -> String {
        x.to_string()
    }

    #[test]
    fn steps_to_text_folds_quick_taps() {
        let steps = vec![Down(s("left_shift")), Down(s("h")), Up(s("h")), Up(s("left_shift")), Delay(120), Down(s("i")), Up(s("i"))];
        assert_eq!(to_text(&steps), "down left_shift\ntap h\nup left_shift\nwait 120\ntap i\n");
    }

    #[test]
    fn text_parses_back_to_steps() {
        let text = "Down Left_Shift\n  tap H \n\nup left_shift\nwait 120\ntap i\n";
        assert_eq!(
            parse_text(text),
            Ok(vec![Down(s("left_shift")), Down(s("h")), Up(s("h")), Up(s("left_shift")), Delay(120), Down(s("i")), Up(s("i"))])
        );
    }

    #[test]
    fn round_trip() {
        let steps = vec![Down(s("a")), Delay(30), Up(s("a")), Down(s("b")), Up(s("b"))];
        assert_eq!(parse_text(&to_text(&steps)), Ok(steps));
    }

    #[test]
    fn errors_name_the_line() {
        assert_eq!(parse_text("tap a\ntap shft"), Err("line 2: unknown key \"shft\"".into()));
        assert_eq!(parse_text("wait soon"), Err("line 1: wait needs a number of milliseconds".into()));
        assert_eq!(parse_text("jump a"), Err("line 1: unknown step \"jump\" (use down, up, tap or wait)".into()));
        assert_eq!(parse_text("tap"), Err("line 1: tap needs a key".into()));
        assert_eq!(parse_text("down left_shift\ntap a"), Err("left_shift is pressed but never released".into()));
        assert_eq!(parse_text(""), Err("the macro is empty".into()));
    }
}
