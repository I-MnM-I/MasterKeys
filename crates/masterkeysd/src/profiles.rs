//! The profiles in memory: which is active and default, edits, and switching when linked programs run.

use masterkeys_core::LightingState;
use masterkeys_core::process::{ProcessInfo, matches};
pub use masterkeys_core::profile::{Profile, Settings};
use masterkeys_core::profile::validate_name;
use std::collections::BTreeMap;
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
}

impl Profiles {
    /// Build from what was on disk. With no profiles, creates `Default`; a missing default falls back
    /// to the first profile by name; starts on the saved active profile if it still exists.
    pub fn from_loaded(loaded: Vec<Profile>, settings: Settings) -> Self {
        let mut pending = Vec::new();
        let mut profiles: BTreeMap<String, Profile> = loaded.into_iter().map(|p| (p.name.clone(), p)).collect();
        if profiles.is_empty() {
            let first = Profile { name: FIRST_RUN_NAME.into(), lighting: LightingState::default(), executables: vec![] };
            profiles.insert(first.name.clone(), first);
            pending.push(StoreOp::Save(FIRST_RUN_NAME.into()));
        }
        let first_name = profiles.keys().next().cloned().expect("at least one profile");
        let default = settings.default_profile.filter(|d| profiles.contains_key(d)).unwrap_or(first_name);
        let active = settings.active_profile.filter(|a| profiles.contains_key(a)).unwrap_or_else(|| default.clone());
        pending.push(StoreOp::Settings);
        Self { profiles, active, default, pending }
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
        Settings { default_profile: Some(self.default.clone()), active_profile: Some(self.active.clone()) }
    }

    /// Disk changes made since the last call.
    pub fn take_pending(&mut self) -> Vec<StoreOp> {
        std::mem::take(&mut self.pending)
    }

    fn known(&self, name: &str) -> Result<(), ProfileError> {
        if self.profiles.contains_key(name) { Ok(()) } else { Err(ProfileError::Invalid(format!("no profile named {name:?}"))) }
    }

    fn new_name(&self, name: &str) -> Result<(), ProfileError> {
        validate_name(name).map_err(ProfileError::Invalid)?;
        if self.profiles.contains_key(name) {
            return Err(ProfileError::Invalid(format!("a profile named {name:?} already exists")));
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
        let profile = Profile { name: name.into(), lighting: self.active().lighting.clone(), executables: vec![] };
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
}

impl AutoSwitch {
    /// The profile to switch to, if the set of running linked programs changed: the most recently
    /// started one's, or the default when none is left. `None` when nothing changed, so a manual
    /// choice holds.
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
        Some(newest.map_or_else(|| profiles.default_name().to_owned(), |(_, _, name)| name.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steelseries_gg::rgb::{Color, Effect};

    fn profile(name: &str, links: &[&str]) -> Profile {
        Profile { name: name.into(), lighting: LightingState::default(), executables: links.iter().map(|s| s.to_string()).collect() }
    }

    fn three() -> Profiles {
        Profiles::from_loaded(
            vec![profile("Default", &[]), profile("Elden", &["eldenring.exe"]), profile("Factorio", &["factorio"])],
            Settings { default_profile: Some("Default".into()), active_profile: None },
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
            Settings { default_profile: Some("Default".into()), active_profile: Some("gaming".into()) },
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
    fn same_pid_restarted_counts_as_new() {
        let p = three();
        let mut a = AutoSwitch::default();
        assert!(a.update(&p, &[proc(5, 10, "eldenring.exe")]).is_some());
        assert!(a.update(&p, &[proc(5, 99, "eldenring.exe")]).is_some());
    }

    #[test]
    fn unlinked_processes_are_ignored() {
        let p = three();
        let mut a = AutoSwitch::default();
        assert_eq!(a.update(&p, &[proc(7, 1, "firefox")]), None);
    }
}
