//! What each key in the drawing should look like.

use masterkeys_core::layout::Layout;
use masterkeys_core::profile::ProfileSummary;
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::Color;

/// Each key's preview colour, in layout order: its zone's colour scaled by brightness (0-100).
pub fn key_colors(layout: &Layout, frame: &[Color; ZONE_COUNT], brightness: u8) -> Vec<Color> {
    let scale = f32::from(brightness.min(100)) / 100.0;
    layout.keys.iter().map(|k| frame[usize::from(k.zone).min(ZONE_COUNT - 1)].scale(scale)).collect()
}

/// Which profile the panel should select after the daemon sends a new list: the new name of a
/// rename once the list shows it done, else the current selection if it still exists, else the
/// active profile.
pub fn reselect(
    selected: Option<&str>,
    pending_rename: Option<&(String, String)>,
    list: &[ProfileSummary],
    active: &str,
) -> Option<String> {
    let exists = |name: &str| list.iter().any(|p| p.name == name);
    if let Some((old, new)) = pending_rename
        && selected == Some(old.as_str())
        && !exists(old)
        && exists(new)
    {
        return Some(new.clone());
    }
    selected.filter(|s| exists(s)).or(Some(active).filter(|a| exists(a))).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use masterkeys_core::profile::ProfileSummary;

    fn summaries(names: &[&str]) -> Vec<ProfileSummary> {
        names.iter().map(|n| ProfileSummary { name: n.to_string(), executables: vec![], is_default: false }).collect()
    }

    #[test]
    fn selection_follows_a_rename_once_the_daemon_lists_it() {
        let pending = Some(("A".to_string(), "C".to_string()));
        assert_eq!(reselect(Some("A"), pending.as_ref(), &summaries(&["B", "C"]), "B").as_deref(), Some("C"));
    }

    #[test]
    fn selection_stays_put_while_a_rename_is_unconfirmed() {
        let pending = Some(("A".to_string(), "B".to_string()));
        assert_eq!(reselect(Some("A"), pending.as_ref(), &summaries(&["A", "B"]), "B").as_deref(), Some("A"));
    }

    #[test]
    fn deleted_selection_falls_back_to_the_active_profile() {
        assert_eq!(reselect(Some("A"), None, &summaries(&["B"]), "B").as_deref(), Some("B"));
    }

    use masterkeys_core::layout::Layout;
    use steelseries_gg::apex3::ZONE_COUNT;
    use steelseries_gg::rgb::Color;

    #[test]
    fn keys_take_their_zone_colour_scaled_by_brightness() {
        let l = Layout::apex3_ansi();
        let mut frame = [Color::BLACK; ZONE_COUNT];
        frame[0] = Color::new(200, 100, 0);
        let c = key_colors(&l, &frame, 50);
        let esc = l.keys.iter().position(|k| k.code == 1).unwrap();
        assert_eq!(c[esc], Color::new(100, 50, 0)); // Esc is in zone 0
        assert_eq!(c.len(), l.keys.len());
    }
}
