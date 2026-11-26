//! The system tray icon: active profile, quick profile switch, open window, quit.

/// What the tray shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrayModel {
    pub profile: String,
    pub profiles: Vec<String>,
    pub connected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TrayItem {
    Header(String),
    Profile { name: String, active: bool },
    Separator,
    Open,
    Quit,
}

pub fn menu_items(m: &TrayModel) -> Vec<TrayItem> {
    let mut items = Vec::new();
    if m.connected {
        items.push(TrayItem::Header(format!("MasterKeys — {}", m.profile)));
        items.extend(m.profiles.iter().map(|name| TrayItem::Profile { name: name.clone(), active: *name == m.profile }));
    } else {
        items.push(TrayItem::Header("MasterKeys — daemon not running".into()));
    }
    items.extend([TrayItem::Separator, TrayItem::Open, TrayItem::Quit]);
    items
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    HideToTray,
    Quit,
}

/// Closing the window hides it only when the tray can bring it back; otherwise the GUI quits.
pub fn on_close(tray_ok: bool) -> CloseAction {
    if tray_ok { CloseAction::HideToTray } else { CloseAction::Quit }
}

#[cfg(target_os = "linux")]
#[path = "tray_linux.rs"]
mod imp;
#[cfg(windows)]
#[path = "tray_windows.rs"]
mod imp;
pub use imp::{TrayHandle, spawn_tray};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_lists_profiles_with_the_active_one_checked() {
        let m = TrayModel { profile: "B".into(), profiles: vec!["A".into(), "B".into()], connected: true };
        assert_eq!(
            menu_items(&m),
            vec![
                TrayItem::Header("MasterKeys — B".into()),
                TrayItem::Profile { name: "A".into(), active: false },
                TrayItem::Profile { name: "B".into(), active: true },
                TrayItem::Separator,
                TrayItem::Open,
                TrayItem::Quit,
            ]
        );
    }

    #[test]
    fn closing_hides_only_when_there_is_a_tray() {
        assert_eq!(on_close(true), CloseAction::HideToTray);
        assert_eq!(on_close(false), CloseAction::Quit);
    }

    #[test]
    fn menu_without_daemon_has_no_profiles() {
        let m = TrayModel { profile: "B".into(), profiles: vec!["A".into()], connected: false };
        assert_eq!(
            menu_items(&m),
            vec![TrayItem::Header("MasterKeys — daemon not running".into()), TrayItem::Separator, TrayItem::Open, TrayItem::Quit]
        );
    }
}
