//! Telling games where the GameSense server is: `coreProps.json`, in `/tmp` and in every Steam
//! Proton prefix (Windows games look under `C:\ProgramData\SteelSeries\SteelSeries Engine 3`).

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

/// Where native SDKs look (the fork's Linux location).
pub const TMP_PATH: &str = "/tmp/steelseries-engine/coreProps.json";

/// Steam's own folders plus every library listed in their `libraryfolders.vdf`.
pub fn steam_libraries(home: &Path) -> Vec<PathBuf> {
    let mut libs: Vec<PathBuf> = Vec::new();
    let mut seen = Vec::new();
    let mut add = |path: PathBuf, libs: &mut Vec<PathBuf>| {
        let Ok(real) = path.canonicalize() else { return };
        if !seen.contains(&real) {
            seen.push(real);
            libs.push(path);
        }
    };
    for root in [home.join(".local/share/Steam"), home.join(".steam/steam")] {
        add(root.clone(), &mut libs);
        let Ok(vdf) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) else { continue };
        for line in vdf.lines() {
            let mut parts = line.split('"').filter(|p| !p.trim().is_empty());
            if parts.next() == Some("path")
                && let Some(path) = parts.next()
            {
                add(PathBuf::from(path.replace("\\\\", "\\")), &mut libs);
            }
        }
    }
    libs
}

/// `/tmp` plus the coreProps location inside every Proton prefix that has a `drive_c`.
pub fn targets(home: &Path) -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from(TMP_PATH)];
    for lib in steam_libraries(home) {
        let Ok(entries) = fs::read_dir(lib.join("steamapps/compatdata")) else { continue };
        let mut prefixes: Vec<PathBuf> =
            entries.flatten().map(|e| e.path().join("pfx/drive_c")).filter(|p| p.is_dir()).collect();
        prefixes.sort();
        out.extend(prefixes.into_iter().map(|c| c.join("ProgramData/SteelSeries/SteelSeries Engine 3/coreProps.json")));
    }
    out
}

/// Write the address to every target; returns a warning per target that failed.
pub fn write_all(home: &Path, port: u16) -> Vec<String> {
    write_targets(&targets(home), port)
}

pub fn write_targets(targets: &[PathBuf], port: u16) -> Vec<String> {
    let text = serde_json::json!({ "address": format!("127.0.0.1:{port}") }).to_string();
    targets.iter().filter_map(|t| write_one(t, &text).err().map(|e| format!("{}: {e}", t.display()))).collect()
}

fn write_one(target: &Path, text: &str) -> std::io::Result<()> {
    if fs::read_to_string(target).is_ok_and(|old| old == text) {
        return Ok(());
    }
    let dir = target.parent().expect("targets have a folder");
    if target.starts_with("/tmp") {
        // A shared folder: create it private, and never write through someone else's or a symlink.
        match fs::symlink_metadata(dir) {
            Ok(meta) if !meta.is_dir() || meta.uid() != fs::metadata("/proc/self")?.uid() => {
                return Err(std::io::Error::other("folder is a symlink or not ours"));
            }
            Ok(_) => {}
            Err(_) => fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?,
        }
    } else {
        fs::create_dir_all(dir)?;
    }
    let tmp = target.with_extension("json.tmp");
    let _ = fs::remove_file(&tmp);
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    file.write_all(text.as_bytes())?;
    fs::rename(&tmp, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_libraries_and_prefixes() {
        let home = tempfile::tempdir().unwrap();
        let steam = home.path().join(".local/share/Steam");
        let lib2 = home.path().join("lib2");
        fs::create_dir_all(steam.join("steamapps")).unwrap();
        fs::write(
            steam.join("steamapps/libraryfolders.vdf"),
            format!("\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n", lib2.display()),
        )
        .unwrap();
        fs::create_dir_all(lib2.join("steamapps/compatdata/100/pfx/drive_c")).unwrap();
        fs::create_dir_all(lib2.join("steamapps/compatdata/200")).unwrap();
        let libs = steam_libraries(home.path());
        assert!(libs.contains(&steam) && libs.contains(&lib2), "{libs:?}");
        let t = targets(home.path());
        assert_eq!(t[0], PathBuf::from(TMP_PATH));
        assert_eq!(t.len(), 2, "{t:?}");
        assert!(t[1].ends_with("compatdata/100/pfx/drive_c/ProgramData/SteelSeries/SteelSeries Engine 3/coreProps.json"));
    }

    #[test]
    fn writes_address_once() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pfx/drive_c/ProgramData/SteelSeries/SteelSeries Engine 3/coreProps.json");
        assert!(write_targets(std::slice::from_ref(&target), 51340).is_empty());
        let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(json["address"], "127.0.0.1:51340");
        let before = fs::metadata(&target).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_targets(std::slice::from_ref(&target), 51340);
        assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), before, "unchanged file not rewritten");
    }
}
