//! Key remaps and macros: how profiles store them and how they compile for the input engine.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;
use steelseries_gg::input::{Action, BindingSet, ESCAPE_CHORD, InputKey, MAX_COMBO_LEN};

pub const MAX_MACRO_STEPS: usize = 500;
pub const MAX_DELAY_MS: u32 = 10_000;
pub const MAX_REPEAT_COUNT: u32 = 1000;

/// One step of a macro, with keys by name (see `InputKey::parse`) and delays in milliseconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MacroStep {
    Down(String),
    Up(String),
    Delay(u32),
}

/// How a macro plays when its key is pressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MacroMode {
    /// Once per press.
    #[default]
    Once,
    /// Over and over while the key is held.
    WhileHeld,
    /// Press to start repeating forever, press again to stop.
    Toggle,
    /// `count` times per press.
    Count,
}

impl MacroMode {
    pub fn is_once(&self) -> bool {
        *self == MacroMode::Once
    }
}

fn one() -> u32 {
    1
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Macro {
    pub steps: Vec<MacroStep>,
    #[serde(default, skip_serializing_if = "MacroMode::is_once")]
    pub mode: MacroMode,
    /// Runs per press in `Count` mode.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub count: u32,
    /// Pause between repeats, in milliseconds.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gap_ms: u32,
}

impl Default for Macro {
    fn default() -> Self {
        Macro { steps: Vec::new(), mode: MacroMode::Once, count: 1, gap_ms: 0 }
    }
}

/// A macro ready to play.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledMacro {
    pub steps: Vec<ResolvedStep>,
    pub mode: MacroMode,
    pub count: u32,
    pub gap: Duration,
}

/// A macro step ready to play.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResolvedStep {
    Down(InputKey),
    Up(InputKey),
    Delay(Duration),
}

/// The active profile's bindings, ready for the input engine.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Compiled {
    pub remaps: BindingSet,
    pub macros: HashMap<InputKey, CompiledMacro>,
}

impl Compiled {
    pub fn is_empty(&self) -> bool {
        self.remaps.is_empty() && self.macros.is_empty()
    }
}

fn parse_key(name: &str) -> Result<InputKey, String> {
    InputKey::parse(name.trim()).map_err(|_| format!("unknown key {:?}", name.trim()))
}

/// The canonical name of a key name, e.g. `"Escape"` gives `"esc"`.
pub fn canonical_key(name: &str) -> Result<String, String> {
    let key = parse_key(name)?;
    Ok(key.name().map_or_else(|| name.trim().to_string(), str::to_string))
}

/// A remap target in canonical spelling, e.g. `"Left_Ctrl + C"` gives `"left_ctrl+c"`.
pub fn canonical_target(text: &str) -> Result<String, String> {
    let keys = match parse_target(text)? {
        Action::Key(k) => vec![k],
        Action::Combo(keys) => keys,
    };
    Ok(keys.iter().map(|k| k.name().map_or_else(|| k.to_string(), str::to_string)).collect::<Vec<_>>().join("+"))
}

/// The keys of the escape chord can't hold macros: a macro swallows its key, and the chord must
/// always be able to stop remapping.
pub fn check_macro_key(key: InputKey) -> Result<(), String> {
    if ESCAPE_CHORD.contains(&key) {
        return Err(format!("{key} is part of the Ctrl+Ctrl+Shift+Shift escape chord and can't hold a macro"));
    }
    Ok(())
}

/// A remap target: one key (`"b"`) or keys pressed together (`"left_ctrl+c"`).
pub fn parse_target(text: &str) -> Result<Action, String> {
    let keys = text.split('+').map(parse_key).collect::<Result<Vec<_>, _>>()?;
    match keys.len() {
        1 => Ok(Action::Key(keys[0])),
        n if n <= MAX_COMBO_LEN => Ok(Action::Combo(keys)),
        _ => Err(format!("a combo can have at most {MAX_COMBO_LEN} keys")),
    }
}

pub fn validate_macro(m: &Macro) -> Result<(), String> {
    compile_macro(m).map(|_| ())
}

fn compile_macro(m: &Macro) -> Result<CompiledMacro, String> {
    if m.mode == MacroMode::Count && !(1..=MAX_REPEAT_COUNT).contains(&m.count) {
        return Err(format!("repeat count must be 1-{MAX_REPEAT_COUNT}"));
    }
    if m.gap_ms > MAX_DELAY_MS {
        return Err(format!("the gap between repeats can be at most {MAX_DELAY_MS} ms"));
    }
    Ok(CompiledMacro {
        steps: resolve(m)?,
        mode: m.mode,
        count: m.count,
        gap: Duration::from_millis(u64::from(m.gap_ms)),
    })
}

fn resolve(m: &Macro) -> Result<Vec<ResolvedStep>, String> {
    if m.steps.is_empty() || m.steps.len() > MAX_MACRO_STEPS {
        return Err(format!("a macro needs 1-{MAX_MACRO_STEPS} steps"));
    }
    let mut held = Vec::new();
    let mut out = Vec::with_capacity(m.steps.len());
    for step in &m.steps {
        out.push(match step {
            MacroStep::Down(name) => {
                let key = parse_key(name)?;
                held.push(key);
                ResolvedStep::Down(key)
            }
            MacroStep::Up(name) => {
                let key = parse_key(name)?;
                let Some(i) = held.iter().position(|k| *k == key) else {
                    return Err(format!("macro releases {name:?} before pressing it"));
                };
                held.remove(i);
                ResolvedStep::Up(key)
            }
            MacroStep::Delay(ms) if *ms > MAX_DELAY_MS => {
                return Err(format!("a macro delay can be at most {MAX_DELAY_MS} ms"));
            }
            MacroStep::Delay(ms) => ResolvedStep::Delay(Duration::from_millis(u64::from(*ms))),
        });
    }
    if let Some(key) = held.first() {
        return Err(format!("macro never releases {key}"));
    }
    Ok(out)
}

/// Compile a profile's bindings; each entry that can't be used is skipped with a warning naming its key.
pub fn compile(remaps: &BTreeMap<String, String>, macros: &BTreeMap<String, Macro>) -> (Compiled, Vec<String>) {
    let mut warnings = Vec::new();
    let mut pairs = Vec::new();
    let mut remap_set = BindingSet::default();
    for (source, target) in remaps {
        let pair = parse_key(source).and_then(|s| parse_target(target).map(|t| (s, t)));
        let candidate = pair.and_then(|pair| {
            let mut next = pairs.clone();
            next.push(pair);
            BindingSet::from_pairs(next.clone()).map(|set| (next, set)).map_err(|e| e.to_string())
        });
        match candidate {
            Ok((next, set)) => {
                pairs = next;
                remap_set = set;
            }
            Err(e) => warnings.push(format!("remap of {source:?} skipped: {e}")),
        }
    }
    let mut resolved = HashMap::new();
    for (source, m) in macros {
        let entry = parse_key(source).and_then(|s| check_macro_key(s).map(|()| s));
        match entry.and_then(|s| compile_macro(m).map(|steps| (s, steps))) {
            Ok((key, steps)) => {
                resolved.insert(key, steps);
            }
            Err(e) => warnings.push(format!("macro on {source:?} skipped: {e}")),
        }
    }
    (Compiled { remaps: remap_set, macros: resolved }, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use steelseries_gg::input::{Action, InputKey};

    fn key(name: &str) -> InputKey {
        InputKey::parse(name).unwrap()
    }

    fn tap(name: &str) -> Macro {
        Macro { steps: vec![MacroStep::Down(name.into()), MacroStep::Delay(30), MacroStep::Up(name.into())], ..Macro::default() }
    }

    #[test]
    fn targets_parse() {
        assert_eq!(parse_target("b"), Ok(Action::Key(key("b"))));
        assert_eq!(parse_target(" Left_Ctrl + c "), Ok(Action::Combo(vec![InputKey::LEFT_CTRL, key("c")])));
        assert!(parse_target("").is_err());
        assert!(parse_target("nope").is_err());
        assert!(parse_target("a+b+c+d+e").is_err());
    }

    #[test]
    fn macros_validate() {
        assert!(validate_macro(&Macro::default()).is_err());
        let long = Macro { steps: vec![MacroStep::Delay(1); 501], ..Macro::default() };
        assert!(validate_macro(&long).is_err());
        assert!(validate_macro(&Macro { steps: vec![MacroStep::Delay(10_001)], ..Macro::default() }).is_err());
        assert!(validate_macro(&Macro { steps: vec![MacroStep::Down("a".into())], ..Macro::default() }).is_err());
        assert!(validate_macro(&Macro { steps: vec![MacroStep::Up("a".into())], ..Macro::default() }).is_err());
        assert!(validate_macro(&Macro { steps: vec![MacroStep::Down("nope".into()), MacroStep::Up("nope".into())], ..Macro::default() }).is_err());
        assert!(validate_macro(&tap("a")).is_ok());
    }

    #[test]
    fn compile_skips_bad_entries_with_warnings() {
        let remaps = BTreeMap::from([
            ("caps_lock".to_string(), "esc".to_string()),
            ("f13".to_string(), "bogus".to_string()),
            ("a".to_string(), "a".to_string()),
        ]);
        let macros = BTreeMap::from([("f1".to_string(), tap("x"))]);
        let (compiled, warnings) = compile(&remaps, &macros);
        assert_eq!(compiled.remaps.len(), 1);
        assert_eq!(compiled.macros.len(), 1);
        assert!(!compiled.is_empty());
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("f13")));
        assert!(warnings.iter().any(|w| w.contains("\"a\"") || w.contains(" a")));
        assert_eq!(
            compiled.macros[&key("f1")].steps,
            vec![ResolvedStep::Down(key("x")), ResolvedStep::Delay(std::time::Duration::from_millis(30)), ResolvedStep::Up(key("x"))]
        );
    }

    #[test]
    fn macros_on_chord_keys_are_skipped() {
        let macros = BTreeMap::from([("left_shift".to_string(), tap("x"))]);
        let (compiled, warnings) = compile(&BTreeMap::new(), &macros);
        assert!(compiled.macros.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn old_macros_load_as_once() {
        let m: Macro = serde_json::from_str(r#"{"steps":[{"down":"a"},{"up":"a"}]}"#).unwrap();
        assert_eq!((m.mode, m.count, m.gap_ms), (MacroMode::Once, 1, 0));
        let json = serde_json::to_string(&m).unwrap();
        assert!(!json.contains("mode") && !json.contains("gap_ms") && !json.contains("count"), "{json}");
    }

    #[test]
    fn repeat_settings_validate() {
        let mut m = tap("a");
        m.mode = MacroMode::Count;
        m.count = 0;
        assert!(validate_macro(&m).is_err());
        m.count = 1001;
        assert!(validate_macro(&m).is_err());
        m.count = 5;
        assert!(validate_macro(&m).is_ok());
        m.gap_ms = 10_001;
        assert!(validate_macro(&m).is_err());
    }

    #[test]
    fn compiled_macros_carry_their_mode() {
        let mut m = tap("x");
        m.mode = MacroMode::Toggle;
        m.gap_ms = 100;
        let (compiled, _) = compile(&BTreeMap::new(), &BTreeMap::from([("f1".to_string(), m)]));
        let c = &compiled.macros[&key("f1")];
        assert_eq!(c.mode, MacroMode::Toggle);
        assert_eq!(c.gap, std::time::Duration::from_millis(100));
        assert_eq!(c.steps.len(), 3);
    }

    #[test]
    fn empty_compiles_empty() {
        assert!(compile(&BTreeMap::new(), &BTreeMap::new()).0.is_empty());
    }
}
