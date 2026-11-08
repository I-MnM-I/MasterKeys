//! Named lighting profiles and where they live on disk.

use crate::LightingState;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use steelseries_gg::rgb::{Color, Effect};

/// A named lighting setup, optionally switched to when linked programs run.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub lighting: LightingState,
    /// Program names that switch to this profile when they start (see `process::matches`).
    pub executables: Vec<String>,
}

/// Which profile is the default and which was active, kept across restarts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub default_profile: Option<String>,
    pub active_profile: Option<String>,
}

/// Profile names double as file names, so keep them safe for that.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.chars().count() > 64 {
        return Err("a profile name needs 1-64 characters".into());
    }
    if name.starts_with('.') || name.contains(['/', '\\', '\0']) {
        return Err(format!("a profile name can't start with '.' or contain / or \\: {name:?}"));
    }
    Ok(())
}

/// The on-disk format.
#[derive(Serialize, Deserialize)]
struct ProfileFile {
    name: String,
    brightness: u8,
    #[serde(default)]
    executables: Vec<String>,
    effect: Effect,
}

/// The format written by the earlier CLI draft: ten zone colours.
#[derive(Deserialize)]
struct LegacyFile {
    name: String,
    brightness: u8,
    zones: Vec<Color>,
}

/// One row of `ListProfiles`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub name: String,
    pub executables: Vec<String>,
    pub is_default: bool,
}

/// Profiles in `<config>/profiles/*.toml`, settings in `<config>/settings.toml`.
#[derive(Clone, Debug)]
pub struct ProfileStore {
    dir: PathBuf,
    settings: PathBuf,
}

impl ProfileStore {
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        let config: PathBuf = config_dir.into();
        Self { dir: config.join("profiles"), settings: config.join("settings.toml") }
    }

    /// `$XDG_CONFIG_HOME/masterkeys`, else `~/.config/masterkeys`.
    pub fn default_config_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|c| c.join("masterkeys"))
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.toml"))
    }

    /// Every readable profile, plus a warning for each file that was skipped.
    pub fn load_all(&self) -> (Vec<Profile>, Vec<String>) {
        let mut profiles = Vec::new();
        let mut warnings = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return (profiles, warnings) };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "toml") {
                continue;
            }
            let file_name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            match load_file(&path) {
                Ok(mut profile) => {
                    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    if profile.name != stem {
                        warnings.push(format!("{file_name}: name {:?} differs from the file name; using {stem:?}", profile.name));
                        profile.name = stem;
                    }
                    profiles.push(profile);
                }
                Err(e) => warnings.push(format!("{file_name}: skipped, {e}")),
            }
        }
        profiles.sort_by(|a, b| a.name.cmp(&b.name));
        (profiles, warnings)
    }

    pub fn save(&self, profile: &Profile) -> io::Result<()> {
        let file = ProfileFile {
            name: profile.name.clone(),
            brightness: profile.lighting.brightness,
            executables: profile.executables.clone(),
            effect: profile.lighting.effect.clone(),
        };
        let text = toml::to_string(&file).map_err(io::Error::other)?;
        write_private(&self.path(&profile.name), &text)
    }

    pub fn delete(&self, name: &str) -> io::Result<()> {
        match std::fs::remove_file(self.path(name)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// Rename a saved profile; fails if `new` already exists.
    pub fn rename(&self, old: &str, new: &str) -> io::Result<()> {
        if self.path(new).exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("profile {new:?} already exists")));
        }
        let mut profile = load_file(&self.path(old))?;
        profile.name = new.into();
        self.save(&profile)?;
        self.delete(old)
    }

    pub fn load_settings(&self) -> Settings {
        std::fs::read_to_string(&self.settings).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        let text = toml::to_string(settings).map_err(io::Error::other)?;
        write_private(&self.settings, &text)
    }
}

fn load_file(path: &Path) -> io::Result<Profile> {
    let text = std::fs::read_to_string(path)?;
    if let Ok(f) = toml::from_str::<ProfileFile>(&text) {
        return Ok(Profile {
            name: f.name,
            lighting: LightingState { effect: f.effect, brightness: f.brightness },
            executables: f.executables,
        });
    }
    match toml::from_str::<LegacyFile>(&text) {
        Ok(f) => Ok(Profile {
            name: f.name,
            lighting: LightingState { effect: Effect::Custom { colors: f.zones }, brightness: f.brightness },
            executables: Vec::new(),
        }),
        // Report the error for the current format, which is what a hand edit is aiming at.
        Err(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            toml::from_str::<ProfileFile>(&text).err().map_or_else(String::new, |e| e.to_string()),
        )),
    }
}

/// Write via a 0600 temp file and rename, so a crash never leaves half a file.
fn write_private(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LightingState;
    use std::os::unix::fs::PermissionsExt;
    use steelseries_gg::rgb::{Color, Effect};

    fn custom(name: &str) -> Profile {
        Profile {
            name: name.into(),
            lighting: LightingState { effect: Effect::Custom { colors: vec![Color::RED; 10] }, brightness: 60 },
            executables: vec!["eldenring.exe".into(), "factorio".into()],
        }
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path());
        store.save(&custom("Elden")).unwrap();
        let (profiles, warnings) = store.load_all();
        assert_eq!(profiles, vec![custom("Elden")]);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn legacy_file_loads_as_custom() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path());
        std::fs::create_dir_all(dir.path().join("profiles")).unwrap();
        let mut legacy = String::from("name = \"gaming\"\nbrightness = 80\n");
        for i in 0..10u8 {
            legacy += &format!("\n[[zones]]\nr = {}\ng = 0\nb = {}\n", i * 20, 255 - i * 20);
        }
        std::fs::write(dir.path().join("profiles/gaming.toml"), legacy).unwrap();
        let (profiles, warnings) = store.load_all();
        assert!(warnings.is_empty(), "{warnings:?}");
        let want: Vec<Color> = (0..10u8).map(|i| Color::new(i * 20, 0, 255 - i * 20)).collect();
        assert_eq!(
            profiles,
            vec![Profile {
                name: "gaming".into(),
                lighting: LightingState { effect: Effect::Custom { colors: want }, brightness: 80 },
                executables: vec![],
            }]
        );
    }

    #[test]
    fn bad_file_is_skipped_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path());
        store.save(&custom("Good")).unwrap();
        let bad = dir.path().join("profiles/x.toml");
        std::fs::write(&bad, "not = [toml").unwrap();
        let (profiles, warnings) = store.load_all();
        assert_eq!(profiles.len(), 1);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("x.toml"), "{warnings:?}");
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), "not = [toml");
    }

    #[test]
    fn names_are_validated() {
        for bad in ["", ".hidden", "a/b", "a\\b", "a\0b", &"x".repeat(65)] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        assert!(validate_name("Elden Ring").is_ok());
    }

    #[test]
    fn rename_refuses_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path());
        store.save(&custom("a")).unwrap();
        store.save(&custom("b")).unwrap();
        assert!(store.rename("a", "b").is_err());
        let (profiles, _) = store.load_all();
        assert_eq!(profiles.len(), 2);
    }

    #[test]
    fn rename_moves_the_file_and_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path());
        store.save(&custom("a")).unwrap();
        store.rename("a", "c").unwrap();
        let (profiles, warnings) = store.load_all();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(profiles.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["c"]);
    }

    #[test]
    fn saves_are_private_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path());
        store.save(&custom("a")).unwrap();
        let mode = std::fs::metadata(dir.path().join("profiles/a.toml")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn settings_round_trip_and_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path());
        assert_eq!(store.load_settings(), Settings::default());
        let s = Settings { default_profile: Some("Default".into()), active_profile: Some("gaming".into()) };
        store.save_settings(&s).unwrap();
        assert_eq!(store.load_settings(), s);
    }
}
