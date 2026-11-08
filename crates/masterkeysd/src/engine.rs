//! The input engine: remaps (the fork's `Remapper`) plus macro playback, and the macro recorder.
//! Pure state machines; `input` feeds them events and times.

use masterkeys_core::bindings::{Compiled, MAX_DELAY_MS, MAX_MACRO_STEPS, Macro, MacroStep, ResolvedStep};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;
use steelseries_gg::input::{InputKey, KeyEvent, KeyValue, Remapper, RemapperConfig};

/// A macro step due at a time, and the macro key that queued it.
struct Queued {
    due: Instant,
    event: KeyEvent,
    source: InputKey,
}

pub struct Engine {
    remapper: Remapper,
    macros: HashMap<InputKey, Vec<ResolvedStep>>,
    queue: VecDeque<Queued>,
    /// Keys the macros currently hold down on the virtual device.
    macro_down: Vec<InputKey>,
    /// Macro keys physically held; their repeats and releases are swallowed even if the macro goes.
    swallowed: HashSet<InputKey>,
}

impl Engine {
    pub fn new(compiled: &Compiled) -> Engine {
        Engine {
            remapper: Remapper::new(&compiled.remaps, RemapperConfig::default()).expect("valid remapper config"),
            macros: compiled.macros.clone(),
            queue: VecDeque::new(),
            macro_down: Vec::new(),
            swallowed: HashSet::new(),
        }
    }

    /// New bindings. Keys already held keep their old output and a playing macro finishes.
    pub fn replace(&mut self, compiled: &Compiled) {
        self.remapper.replace_bindings(&compiled.remaps);
        self.macros = compiled.macros.clone();
    }

    /// False once the escape chord has fired.
    pub fn is_active(&self) -> bool {
        self.remapper.is_active()
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        if !self.is_active() {
            return None;
        }
        let next_step = self.queue.front().map(|q| q.due);
        match (self.remapper.next_deadline(), next_step) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    pub fn handle(&mut self, event: KeyEvent, now: Instant) -> Vec<KeyEvent> {
        let mut out = self.tick(now);
        if !self.is_active() {
            return out;
        }
        let is_macro_event = match event.value {
            KeyValue::Press => self.macros.contains_key(&event.key) || self.swallowed.contains(&event.key),
            KeyValue::Repeat | KeyValue::Release => self.swallowed.contains(&event.key),
        };
        if !is_macro_event {
            out.extend(self.remapper.handle(event, now));
            return out;
        }
        match event.value {
            KeyValue::Press => {
                self.swallowed.insert(event.key);
                let playing = self.queue.iter().any(|q| q.source == event.key);
                if !playing && let Some(steps) = self.macros.get(&event.key).cloned() {
                    self.schedule(event.key, &steps, now);
                    out.extend(self.play_due(now));
                }
            }
            KeyValue::Repeat => {}
            KeyValue::Release => {
                self.swallowed.remove(&event.key);
            }
        }
        out
    }

    /// Run due macro steps, and the escape chord's timer.
    pub fn tick(&mut self, now: Instant) -> Vec<KeyEvent> {
        let mut out = self.remapper.tick(now);
        if !self.remapper.is_active() {
            if !self.queue.is_empty() || !self.macro_down.is_empty() {
                out.extend(self.abort_macros());
            }
            return out;
        }
        out.extend(self.play_due(now));
        out
    }

    /// Release every key held down on the virtual device and forget all held and queued keys.
    pub fn release_all(&mut self) -> Vec<KeyEvent> {
        let mut out = self.abort_macros();
        out.extend(self.remapper.release_all());
        self.swallowed.clear();
        out
    }

    fn abort_macros(&mut self) -> Vec<KeyEvent> {
        self.queue.clear();
        self.macro_down.drain(..).rev().map(|key| KeyEvent { key, value: KeyValue::Release }).collect()
    }

    /// Queue a macro after anything already queued.
    fn schedule(&mut self, source: InputKey, steps: &[ResolvedStep], now: Instant) {
        let mut at = self.queue.back().map_or(now, |q| q.due.max(now));
        for step in steps {
            let (key, value) = match *step {
                ResolvedStep::Delay(d) => {
                    at += d;
                    continue;
                }
                ResolvedStep::Down(key) => (key, KeyValue::Press),
                ResolvedStep::Up(key) => (key, KeyValue::Release),
            };
            self.queue.push_back(Queued { due: at, event: KeyEvent { key, value }, source });
        }
    }

    fn play_due(&mut self, now: Instant) -> Vec<KeyEvent> {
        let mut out = Vec::new();
        while self.queue.front().is_some_and(|q| q.due <= now) {
            let q = self.queue.pop_front().expect("front exists");
            match q.event.value {
                KeyValue::Press => self.macro_down.push(q.event.key),
                _ => self.macro_down.retain(|k| *k != q.event.key),
            }
            out.push(q.event);
        }
        out
    }
}

/// Records physical key transitions into a macro, with the real time between them.
pub struct Recorder {
    started: Instant,
    steps: Vec<MacroStep>,
    last: Option<Instant>,
    held: Vec<InputKey>,
}

impl Recorder {
    pub fn start(now: Instant) -> Recorder {
        Recorder { started: now, steps: Vec::new(), last: None, held: Vec::new() }
    }

    pub fn started(&self) -> Instant {
        self.started
    }

    pub fn feed(&mut self, code: u16, pressed: bool, at: Instant) {
        let key = InputKey::from_code(code);
        let Some(name) = key.name() else { return };
        if !pressed && !self.held.contains(&key) {
            return;
        }
        // Room for a delay, this step and the releases `finish` may add.
        let closing = self.held.len() + usize::from(pressed);
        if self.steps.len() + 2 + closing > MAX_MACRO_STEPS {
            return;
        }
        if let Some(last) = self.last {
            let ms = at.saturating_duration_since(last).as_secs_f64() * 1000.0;
            let ms = ms.round().min(f64::from(MAX_DELAY_MS)) as u32;
            if ms >= 1 {
                self.steps.push(MacroStep::Delay(ms));
            }
        }
        self.last = Some(at);
        if pressed {
            if !self.held.contains(&key) {
                self.held.push(key);
                self.steps.push(MacroStep::Down(name.into()));
            }
        } else {
            self.held.retain(|k| *k != key);
            self.steps.push(MacroStep::Up(name.into()));
        }
    }

    /// The macro, with releases added for keys still held.
    pub fn finish(mut self) -> Macro {
        for key in self.held.drain(..).rev() {
            self.steps.push(MacroStep::Up(key.name().expect("recorded keys have names").into()));
        }
        Macro { steps: self.steps }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use masterkeys_core::bindings::{Macro, MacroStep, compile, validate_macro};
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};
    use steelseries_gg::input::{InputKey, KeyEvent, KeyValue};

    fn k(name: &str) -> InputKey {
        InputKey::parse(name).unwrap()
    }
    fn press(name: &str) -> KeyEvent {
        KeyEvent { key: k(name), value: KeyValue::Press }
    }
    fn repeat(name: &str) -> KeyEvent {
        KeyEvent { key: k(name), value: KeyValue::Repeat }
    }
    fn release(name: &str) -> KeyEvent {
        KeyEvent { key: k(name), value: KeyValue::Release }
    }
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }
    fn engine(remaps: &[(&str, &str)], macros: &[(&str, Vec<MacroStep>)]) -> Engine {
        let remaps: BTreeMap<String, String> = remaps.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let macros: BTreeMap<String, Macro> =
            macros.iter().map(|(a, steps)| (a.to_string(), Macro { steps: steps.clone() })).collect();
        let (compiled, warnings) = compile(&remaps, &macros);
        assert!(warnings.is_empty(), "{warnings:?}");
        Engine::new(&compiled)
    }
    fn down(n: &str) -> MacroStep {
        MacroStep::Down(n.into())
    }
    fn up(n: &str) -> MacroStep {
        MacroStep::Up(n.into())
    }
    fn hi() -> Vec<MacroStep> {
        vec![down("a"), MacroStep::Delay(50), up("a"), down("b"), up("b")]
    }

    #[test]
    fn unbound_keys_pass_through() {
        let mut e = engine(&[], &[]);
        let t = Instant::now();
        assert_eq!(e.handle(press("a"), t), vec![press("a")]);
        assert_eq!(e.handle(repeat("a"), t), vec![repeat("a")]);
        assert_eq!(e.handle(release("a"), t), vec![release("a")]);
    }

    #[test]
    fn remap_and_combo() {
        let mut e = engine(&[("caps_lock", "esc"), ("f13", "left_ctrl+c")], &[]);
        let t = Instant::now();
        assert_eq!(e.handle(press("caps_lock"), t), vec![press("esc")]);
        assert_eq!(e.handle(release("caps_lock"), t), vec![release("esc")]);
        assert_eq!(e.handle(press("f13"), t), vec![press("left_ctrl"), press("c")]);
        assert_eq!(e.handle(release("f13"), t), vec![release("c"), release("left_ctrl")]);
    }

    #[test]
    fn release_after_rebind_releases_original_output() {
        let mut e = engine(&[("caps_lock", "esc")], &[]);
        let t = Instant::now();
        assert_eq!(e.handle(press("caps_lock"), t), vec![press("esc")]);
        e.replace(&compile(&BTreeMap::new(), &BTreeMap::new()).0);
        assert_eq!(e.handle(release("caps_lock"), t), vec![release("esc")]);
    }

    #[test]
    fn macro_plays_with_delays() {
        let mut e = engine(&[], &[("f1", hi())]);
        let t = Instant::now();
        assert_eq!(e.handle(press("f1"), t), vec![press("a")]);
        assert_eq!(e.next_deadline(), Some(t + ms(50)));
        assert_eq!(e.tick(t + ms(49)), vec![]);
        assert_eq!(e.tick(t + ms(50)), vec![release("a"), press("b"), release("b")]);
        assert_eq!(e.handle(release("f1"), t + ms(60)), vec![]);
        assert_eq!(e.next_deadline(), None);
    }

    #[test]
    fn macro_key_repeat_and_repress_dont_retrigger() {
        let mut e = engine(&[], &[("f1", hi())]);
        let t = Instant::now();
        e.handle(press("f1"), t);
        assert_eq!(e.handle(repeat("f1"), t + ms(10)), vec![]);
        assert_eq!(e.handle(release("f1"), t + ms(20)), vec![]);
        assert_eq!(e.handle(press("f1"), t + ms(30)), vec![]);
        assert_eq!(e.tick(t + ms(50)), vec![release("a"), press("b"), release("b")]);
        assert_eq!(e.next_deadline(), None);
        assert_eq!(e.handle(release("f1"), t + ms(60)), vec![]);
        // Once finished, a new press plays it again.
        assert_eq!(e.handle(press("f1"), t + ms(70)), vec![press("a")]);
    }

    #[test]
    fn other_keys_pass_during_macro() {
        let mut e = engine(&[], &[("f1", hi())]);
        let t = Instant::now();
        e.handle(press("f1"), t);
        assert_eq!(e.handle(press("x"), t + ms(10)), vec![press("x")]);
        assert_eq!(e.handle(release("x"), t + ms(20)), vec![release("x")]);
    }

    #[test]
    fn release_all_mid_macro_releases_its_keys() {
        let mut e = engine(&[("caps_lock", "esc")], &[("f1", hi())]);
        let t = Instant::now();
        e.handle(press("caps_lock"), t);
        e.handle(press("f1"), t);
        let out = e.release_all();
        assert!(out.contains(&release("a")), "{out:?}");
        assert!(out.contains(&release("esc")), "{out:?}");
        assert_eq!(e.next_deadline(), None);
        assert_eq!(e.tick(t + ms(100)), vec![]);
    }

    #[test]
    fn escape_chord_stops_engine() {
        let mut e = engine(&[("caps_lock", "esc")], &[]);
        let t = Instant::now();
        for key in ["left_ctrl", "right_ctrl", "left_shift", "right_shift"] {
            e.handle(press(key), t);
        }
        assert!(e.is_active());
        let out = e.tick(t + Duration::from_secs(2));
        assert!(out.contains(&release("left_ctrl")), "{out:?}");
        assert!(!e.is_active());
    }

    #[test]
    fn recorder_captures_timing() {
        let t = Instant::now();
        let mut r = Recorder::start(t);
        r.feed(30, true, t);
        r.feed(30, false, t + ms(30));
        r.feed(48, true, t + ms(30));
        r.feed(48, false, t + ms(80));
        assert_eq!(r.finish().steps, vec![down("a"), MacroStep::Delay(30), up("a"), down("b"), MacroStep::Delay(50), up("b")]);
    }

    #[test]
    fn recorder_caps_delays_and_closes_held_keys() {
        let t = Instant::now();
        let mut r = Recorder::start(t);
        r.feed(30, true, t);
        r.feed(48, true, t + Duration::from_secs(20));
        let m = r.finish();
        assert_eq!(m.steps[1], MacroStep::Delay(10_000));
        assert!(m.steps.ends_with(&[up("a"), up("b")]) || m.steps.ends_with(&[up("b"), up("a")]), "{:?}", m.steps);
        assert!(validate_macro(&m).is_ok());
    }

    #[test]
    fn recorder_skips_orphan_releases_and_unknown_codes() {
        let t = Instant::now();
        let mut r = Recorder::start(t);
        r.feed(30, false, t);
        r.feed(0, true, t);
        r.feed(48, true, t);
        r.feed(48, false, t);
        assert_eq!(r.finish().steps, vec![down("b"), up("b")]);
    }

    #[test]
    fn recorder_stops_at_the_step_limit() {
        let t = Instant::now();
        let mut r = Recorder::start(t);
        for i in 0..1000u64 {
            r.feed(30, true, t + ms(i * 10));
            r.feed(30, false, t + ms(i * 10 + 5));
        }
        let m = r.finish();
        assert!(m.steps.len() <= 500, "{}", m.steps.len());
        assert!(validate_macro(&m).is_ok());
    }
}
