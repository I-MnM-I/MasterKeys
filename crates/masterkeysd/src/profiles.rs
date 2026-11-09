//! The profiles in memory: which is active and default, edits, and switching when linked programs run.

use masterkeys_core::LightingState;
use masterkeys_core::process::{ProcessInfo, matches};
pub use masterkeys_core::profile::{Profile, Settings};
use masterkeys_core::bindings::{Macro, canonical_key, canonical_target, check_macro_key, parse_target, validate_macro};
use masterkeys_core::profile::validate_name;
use steelseries_gg::input::{BindingSet, InputKey};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

const FIRST_RUN_NAME: &str = "Default";

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileError {
    /// A bad name or an unknown profile.
    Invalid(String),
    /// Allowed in principle, but it would break a rule (e.g. deleting the default).
    Refused(String),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::Invalid(m) | ProfileError::Refused(m) => f.write_str(m),
        }
    }
}

/// A change the saver must make on disk.
#[derive(Clone, Debug, PartialEq)]
pub enum StoreOp {
    Save(String),
    Delete(String),
    Rename(String, String),
    Settings,
}

#[derive(Clone, Debug)]
pub struct Profiles {
    profiles: BTreeMap<String, Profile>,
    active: String,
    default: String,
    pending: Vec<StoreOp>,
    gamesense_disabled: bool,
    /// Names of profile files on disk that couldn't be read; never written over.
    reserved: BTreeSet<String>,
}

impl Profiles {
    /// Build from what was on disk. With no profiles, creates `Default`; a missing default falls back
    /// to the first profile by name; starts on the saved active profile if it still exists.
    pub fn from_loaded(loaded: Vec<Profile>, settings: Settings) -> Self {
        let mut pending = Vec::new();
        let mut profiles: BTreeMap<String, Profile> = loaded.into_iter().map(|p| (p.name.clone(), p)).collect();
        if profiles.is_empty() {
            let first = Profile { name: FIRST_RUN_NAME.into(), ..Profile::default() };
            profiles.insert(first.name.clone(), first);
            pending.push(StoreOp::Save(FIRST_RUN_NAME.into()));
        }
        let first_name = profiles.keys().next().cloned().expect("at least one profile");
        let default = settings.default_profile.filter(|d| profiles.contains_key(d)).unwrap_or(first_name);
        let active = settings.active_profile.filter(|a| profiles.contains_key(a)).unwrap_or_else(|| default.clone());
        pending.push(StoreOp::Settings);
        Self { profiles, active, default, pending, gamesense_disabled: settings.gamesense_disabled, reserved: BTreeSet::new() }
    }

    pub fn active(&self) -> &Profile {
        &self.profiles[&self.active]
    }

    pub fn default_name(&self) -> &str {
        &self.default
    }

    /// All profiles, sorted by name.
    pub fn list(&self) -> Vec<&Profile> {
        self.profiles.values().collect()
    }

    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }

    pub fn settings(&self) -> Settings {
        Settings {
            default_profile: Some(self.default.clone()),
            active_profile: Some(self.active.clone()),
            gamesense_disabled: self.gamesense_disabled,
        }
    }

    pub fn gamesense_enabled(&self) -> bool {
        !self.gamesense_disabled
    }

    pub fn set_gamesense_enabled(&mut self, enabled: bool) {
        if self.gamesense_disabled == enabled {
            self.gamesense_disabled = !enabled;
            self.pending.push(StoreOp::Settings);
        }
    }

    /// Disk changes made since the last call.
    pub fn take_pending(&mut self) -> Vec<StoreOp> {
        std::mem::take(&mut self.pending)
    }

    /// Keep these names (unreadable files on disk) from being created or renamed onto.
    pub fn reserve(&mut self, names: impl IntoIterator<Item = String>) {
        self.reserved.extend(names.into_iter().filter(|n| !self.profiles.contains_key(n)));
    }

    fn known(&self, name: &str) -> Result<(), ProfileError> {
        if self.profiles.contains_key(name) { Ok(()) } else { Err(ProfileError::Invalid(format!("no profile named {name:?}"))) }
    }

    fn new_name(&self, name: &str) -> Result<(), ProfileError> {
        validate_name(name).map_err(ProfileError::Invalid)?;
        if self.profiles.contains_key(name) {
            return Err(ProfileError::Invalid(format!("a profile named {name:?} already exists")));
        }
        if self.reserved.contains(name) {
            return Err(ProfileError::Invalid(format!(
                "profiles/{name}.toml exists but couldn't be read; fix or remove that file first"
            )));
        }
        Ok(())
    }

    pub fn activate(&mut self, name: &str) -> Result<(), ProfileError> {
        self.known(name)?;
        if self.active != name {
            self.active = name.into();
            self.pending.push(StoreOp::Settings);
        }
        Ok(())
    }

    /// Lighting edits always belong to the active profile.
    pub fn set_lighting(&mut self, lighting: LightingState) {
        let active = self.active.clone();
        self.profiles.get_mut(&active).expect("active exists").lighting = lighting;
        self.pending.push(StoreOp::Save(active));
    }

    /// A new profile with the current lighting and no links; does not activate it.
    pub fn create(&mut self, name: &str) -> Result<(), ProfileError> {
        self.new_name(name)?;
        let profile = Profile { name: name.into(), lighting: self.active().lighting.clone(), ..Profile::default() };
        self.profiles.insert(name.into(), profile);
        self.pending.push(StoreOp::Save(name.into()));
        Ok(())
    }

    pub fn delete(&mut self, name: &str) -> Result<(), ProfileError> {
        self.known(name)?;
        if self.profiles.len() == 1 {
            return Err(ProfileError::Refused("can't delete the only profile".into()));
        }
        if self.default == name {
            return Err(ProfileError::Refused("can't delete the default profile; make another profile the default first".into()));
        }
        self.profiles.remove(name);
        if self.active == name {
            self.active = self.default.clone();
        }
        self.pending.push(StoreOp::Delete(name.into()));
        self.pending.push(StoreOp::Settings);
        Ok(())
    }

    pub fn rename(&mut self, old: &str, new: &str) -> Result<(), ProfileError> {
        self.known(old)?;
        self.new_name(new)?;
        let mut profile = self.profiles.remove(old).expect("known");
        profile.name = new.into();
        self.profiles.insert(new.into(), profile);
        for name in [&mut self.active, &mut self.default] {
            if name == old {
                *name = new.into();
            }
        }
        self.pending.push(StoreOp::Rename(old.into(), new.into()));
        self.pending.push(StoreOp::Settings);
        Ok(())
    }

    /// Replace a profile's linked programs: trimmed, empties dropped, duplicates (ignoring case) removed.
    pub fn set_executables(&mut self, name: &str, executables: Vec<String>) -> Result<(), ProfileError> {
        self.known(name)?;
        let mut clean: Vec<String> = Vec::new();
        for exe in executables {
            let exe = exe.trim();
            if !exe.is_empty() && !clean.iter().any(|c| c.eq_ignore_ascii_case(exe)) {
                clean.push(exe.into());
            }
        }
        self.profiles.get_mut(name).expect("known").executables = clean;
        self.pending.push(StoreOp::Save(name.into()));
        Ok(())
    }

    /// Remap a key on the active profile; any macro on that key goes.
    pub fn set_remap(&mut self, key: &str, target: &str) -> Result<(), ProfileError> {
        let source = canonical_key(key).map_err(ProfileError::Invalid)?;
        let action = parse_target(target).map_err(ProfileError::Invalid)?;
        let source_key = InputKey::parse(&source).map_err(|e| ProfileError::Invalid(e.to_string()))?;
        BindingSet::from_pairs([(source_key, action.clone())]).map_err(|e| ProfileError::Invalid(e.to_string()))?;
        let target = canonical_target(target).map_err(ProfileError::Invalid)?;
        let profile = self.active_mut();
        profile.macros.remove(&source);
        profile.remaps.insert(source, target);
        self.save_active();
        Ok(())
    }

    pub fn clear_remap(&mut self, key: &str) -> Result<(), ProfileError> {
        let source = canonical_key(key).map_err(ProfileError::Invalid)?;
        if self.active_mut().remaps.remove(&source).is_some() {
            self.save_active();
        }
        Ok(())
    }

    /// Put a macro on a key of the active profile; any remap of that key goes.
    pub fn set_macro(&mut self, key: &str, m: Macro) -> Result<(), ProfileError> {
        let source = canonical_key(key).map_err(ProfileError::Invalid)?;
        let source_key = InputKey::parse(&source).map_err(|e| ProfileError::Invalid(e.to_string()))?;
        check_macro_key(source_key).map_err(ProfileError::Invalid)?;
        validate_macro(&m).map_err(ProfileError::Invalid)?;
        let profile = self.active_mut();
        profile.remaps.remove(&source);
        profile.macros.insert(source, m);
        self.save_active();
        Ok(())
    }

    pub fn clear_macro(&mut self, key: &str) -> Result<(), ProfileError> {
        let source = canonical_key(key).map_err(ProfileError::Invalid)?;
        if self.active_mut().macros.remove(&source).is_some() {
            self.save_active();
        }
        Ok(())
    }

    fn active_mut(&mut self) -> &mut Profile {
        self.profiles.get_mut(&self.active).expect("active exists")
    }

    fn save_active(&mut self) {
        let active = self.active.clone();
        self.pending.push(StoreOp::Save(active));
    }

    pub fn set_default(&mut self, name: &str) -> Result<(), ProfileError> {
        self.known(name)?;
        self.default = name.into();
        self.pending.push(StoreOp::Settings);
        Ok(())
    }

    /// The profile a process belongs to: the first profile by name with a matching link.
    fn profile_for(&self, process: &ProcessInfo) -> Option<&str> {
        self.profiles.values().find(|p| p.executables.iter().any(|link| matches(process, link))).map(|p| p.name.as_str())
    }
}

/// Decides when linked programs starting or exiting should switch profile.
#[derive(Default)]
pub struct AutoSwitch {
    /// Linked processes seen last time: (pid, start, profile).
    seen: Vec<(u32, u64, String)>,
    /// The profile last switched to, so more processes of the same game don't undo a manual pick.
    target: Option<String>,
}

impl AutoSwitch {
    /// The profile to switch to when the running linked programs change which profile they call
    /// for: the most recently started one's, or the default when none is left. `None` otherwise, so
    /// a manual choice holds.
    pub fn update(&mut self, profiles: &Profiles, running: &[ProcessInfo]) -> Option<String> {
        let mut now: Vec<(u32, u64, String)> = running
            .iter()
            .filter_map(|p| profiles.profile_for(p).map(|name| (p.pid, p.start, name.to_owned())))
            .collect();
        now.sort();
        if now == self.seen {
            return None;
        }
        self.seen = now;
        let newest = self.seen.iter().max_by_key(|(_, start, _)| *start);
        let target = newest.map_or_else(|| profiles.default_name().to_owned(), |(_, _, name)| name.clone());
        if self.target.as_ref() == Some(&target) {
            return None;
        }
        self.target = Some(target.clone());
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steelseries_gg::rgb::{Color, Effect};

    fn profile(name: &str, links: &[&str]) -> Profile {
        Profile { name: name.into(), executables: links.iter().map(|s| s.to_string()).collect(), ..Profile::default() }
    }

    fn three() -> Profiles {
        Profiles::from_loaded(
            vec![profile("Default", &[]), profile("Elden", &["eldenring.exe"]), profile("Factorio", &["factorio"])],
            Settings { default_profile: Some("Default".into()), active_profile: None, ..Settings::default() },
        )
    }

    fn proc(pid: u32, start: u64, exe: &str) -> ProcessInfo {
        ProcessInfo { pid, start, exe: Some(format!("/x/{exe}")), cmdline: vec![] }
    }

    #[test]
    fn first_run_creates_default() {
        let p = Profiles::from_loaded(vec![], Settings::default());
        assert_eq!(p.active().name, "Default");
        assert_eq!(p.default_name(), "Default");
        assert!(p.clone().take_pending().contains(&StoreOp::Save("Default".into())));
    }

    #[test]
    fn starts_on_saved_active_profile() {
        let p = Profiles::from_loaded(
            vec![profile("Default", &[]), profile("gaming", &[])],
            Settings { default_profile: Some("Default".into()), active_profile: Some("gaming".into()), ..Settings::default() },
        );
        assert_eq!(p.active().name, "gaming");
    }

    #[test]
    fn missing_default_falls_back_to_first_by_name() {
        let p = Profiles::from_loaded(vec![profile("b", &[]), profile("a", &[])], Settings::default());
        assert_eq!(p.default_name(), "a");
        assert_eq!(p.active().name, "a");
    }

    #[test]
    fn lighting_edits_go_to_active_profile() {
        let mut p = three();
        p.activate("Elden").unwrap();
        p.take_pending();
        let red = LightingState { effect: Effect::Static { color: Color::RED }, brightness: 50 };
        p.set_lighting(red.clone());
        assert_eq!(p.active().lighting, red);
        assert_eq!(p.list().iter().find(|x| x.name == "Default").unwrap().lighting, LightingState::default());
        assert_eq!(p.take_pending(), vec![StoreOp::Save("Elden".into())]);
    }

    #[test]
    fn delete_rules() {
        let mut p = three();
        assert!(matches!(p.delete("Default"), Err(ProfileError::Refused(_))));
        p.activate("Elden").unwrap();
        p.delete("Elden").unwrap();
        assert_eq!(p.active().name, "Default");
        p.delete("Factorio").unwrap();
        assert!(matches!(p.delete("Default"), Err(ProfileError::Refused(_))), "last profile");
        assert!(matches!(p.delete("nope"), Err(ProfileError::Invalid(_))));
    }

    #[test]
    fn rename_rules() {
        let mut p = three();
        assert!(matches!(p.rename("Elden", "Factorio"), Err(ProfileError::Invalid(_))));
        assert!(matches!(p.rename("Elden", "../x"), Err(ProfileError::Invalid(_))));
        p.take_pending();
        p.rename("Default", "Main").unwrap();
        assert_eq!(p.default_name(), "Main");
        assert_eq!(p.active().name, "Main");
        assert_eq!(p.take_pending(), vec![StoreOp::Rename("Default".into(), "Main".into()), StoreOp::Settings]);
    }

    #[test]
    fn create_copies_current_lighting() {
        let mut p = three();
        let blue = LightingState { effect: Effect::Static { color: Color::BLUE }, brightness: 30 };
        p.set_lighting(blue.clone());
        p.create("Night").unwrap();
        assert_eq!(p.active().name, "Default", "create does not activate");
        assert_eq!(p.list().iter().find(|x| x.name == "Night").unwrap().lighting, blue);
        assert!(matches!(p.create("Night"), Err(ProfileError::Invalid(_))));
    }

    #[test]
    fn duplicate_links_are_cleaned() {
        let mut p = three();
        p.set_executables("Elden", vec!["a.exe".into(), " A.EXE ".into(), "".into(), "b".into()]).unwrap();
        assert_eq!(p.list().iter().find(|x| x.name == "Elden").unwrap().executables, ["a.exe", "b"]);
    }

    #[test]
    fn remap_and_macro_replace_each_other() {
        use masterkeys_core::bindings::{Macro, MacroStep};
        let tap = Macro { steps: vec![MacroStep::Down("a".into()), MacroStep::Up("a".into())] };
        let mut p = three();
        p.take_pending();
        p.set_remap("Caps_Lock", "Escape").unwrap();
        assert_eq!(p.active().remaps.get("caps_lock").map(String::as_str), Some("esc"));
        assert_eq!(p.take_pending(), vec![StoreOp::Save("Default".into())]);
        p.set_macro("caps_lock", tap.clone()).unwrap();
        assert!(p.active().remaps.is_empty());
        assert_eq!(p.active().macros.get("caps_lock"), Some(&tap));
        p.set_remap("caps_lock", "left_ctrl + c").unwrap();
        assert!(p.active().macros.is_empty());
        assert_eq!(p.active().remaps["caps_lock"], "left_ctrl+c");
        p.clear_remap("caps_lock").unwrap();
        p.set_macro("f1", tap).unwrap();
        p.clear_macro("f1").unwrap();
        assert!(p.active().remaps.is_empty() && p.active().macros.is_empty());
    }

    #[test]
    fn chord_keys_cannot_hold_macros() {
        use masterkeys_core::bindings::{Macro, MacroStep};
        let tap = Macro { steps: vec![MacroStep::Down("a".into()), MacroStep::Up("a".into())] };
        let mut p = three();
        for key in ["left_ctrl", "right_ctrl", "left_shift", "right_shift"] {
            assert!(matches!(p.set_macro(key, tap.clone()), Err(ProfileError::Invalid(_))), "{key}");
        }
    }

    #[test]
    fn bad_bindings_are_invalid() {
        use masterkeys_core::bindings::{Macro, MacroStep};
        let mut p = three();
        assert!(matches!(p.set_remap("a", "a"), Err(ProfileError::Invalid(_))));
        assert!(matches!(p.set_remap("nope", "b"), Err(ProfileError::Invalid(_))));
        assert!(matches!(p.set_remap("a", "nope"), Err(ProfileError::Invalid(_))));
        let bad = Macro { steps: vec![MacroStep::Down("a".into())] };
        assert!(matches!(p.set_macro("f1", bad), Err(ProfileError::Invalid(_))));
    }

    #[test]
    fn game_start_and_exit() {
        let p = three();
        let mut a = AutoSwitch::default();
        assert_eq!(a.update(&p, &[]), None);
        assert_eq!(a.update(&p, &[proc(5, 10, "eldenring.exe")]).as_deref(), Some("Elden"));
        assert_eq!(a.update(&p, &[proc(5, 10, "eldenring.exe")]), None);
        assert_eq!(a.update(&p, &[]).as_deref(), Some("Default"));
    }

    #[test]
    fn exit_falls_back_to_other_running_game() {
        let p = three();
        let mut a = AutoSwitch::default();
        let elden = proc(5, 10, "eldenring.exe");
        let factorio = proc(6, 20, "factorio");
        assert_eq!(a.update(&p, std::slice::from_ref(&elden)).as_deref(), Some("Elden"));
        assert_eq!(a.update(&p, &[elden.clone(), factorio]).as_deref(), Some("Factorio"));
        assert_eq!(a.update(&p, &[elden]).as_deref(), Some("Elden"));
    }

    #[test]
    fn manual_pick_holds_until_next_game_event() {
        let mut p = three();
        let mut a = AutoSwitch::default();
        let elden = proc(5, 10, "eldenring.exe");
        assert_eq!(a.update(&p, std::slice::from_ref(&elden)).as_deref(), Some("Elden"));
        p.activate("Factorio").unwrap();
        assert_eq!(a.update(&p, &[elden]), None);
        assert_eq!(a.update(&p, &[]).as_deref(), Some("Default"));
    }

    #[test]
    fn more_processes_of_the_same_game_keep_a_manual_pick() {
        let mut p = three();
        let mut a = AutoSwitch::default();
        let launcher = proc(5, 10, "eldenring.exe");
        let game = proc(6, 20, "eldenring.exe");
        assert_eq!(a.update(&p, std::slice::from_ref(&launcher)).as_deref(), Some("Elden"));
        p.activate("Factorio").unwrap();
        assert_eq!(a.update(&p, &[launcher, game.clone()]), None);
        assert_eq!(a.update(&p, &[game]), None);
        assert_eq!(a.update(&p, &[]).as_deref(), Some("Default"));
    }

    #[test]
    fn skipped_file_names_cannot_be_taken() {
        let mut p = three();
        p.reserve(["broken".to_string()]);
        assert!(matches!(p.create("broken"), Err(ProfileError::Invalid(_))));
        assert!(matches!(p.rename("Elden", "broken"), Err(ProfileError::Invalid(_))));
    }

    #[test]
    fn unlinked_processes_are_ignored() {
        let p = three();
        let mut a = AutoSwitch::default();
        assert_eq!(a.update(&p, &[proc(7, 1, "firefox")]), None);
    }
}
