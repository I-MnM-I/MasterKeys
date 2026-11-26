//! The Windows app's log file, kept small.

use std::io;
use std::path::Path;

/// Move `path` to `<path>.1` (replacing an older one) once it is bigger than `limit` bytes.
pub fn rotate_if_big(path: &Path, limit: u64) -> io::Result<()> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > limit => {
            let mut old = path.as_os_str().to_owned();
            old.push(".1");
            std::fs::rename(path, old)
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_only_past_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("masterkeys.log");
        std::fs::write(&log, "0123456789").unwrap();
        rotate_if_big(&log, 50).unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "0123456789");
        rotate_if_big(&log, 5).unwrap();
        assert!(!log.exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("masterkeys.log.1")).unwrap(), "0123456789");
        std::fs::write(&log, "new").unwrap();
        rotate_if_big(&log, 1).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("masterkeys.log.1")).unwrap(), "new");
    }
}
