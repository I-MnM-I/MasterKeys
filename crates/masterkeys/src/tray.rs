//! The system tray icon: active profile, quick profile switch, open window, quit.

use crate::client::Command;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

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

type Action = Arc<dyn Fn() + Send + Sync>;

pub struct Tray {
    model: TrayModel,
    commands: UnboundedSender<Command>,
    show: Action,
    quit: Action,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "masterkeys".into()
    }

    fn title(&self) -> String {
        "MasterKeys".into()
    }

    fn icon_name(&self) -> String {
        "input-keyboard".into()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        (self.show)();
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{CheckmarkItem, MenuItem, StandardItem};
        menu_items(&self.model)
            .into_iter()
            .map(|item| match item {
                TrayItem::Header(label) => StandardItem { label, enabled: false, ..Default::default() }.into(),
                TrayItem::Profile { name, active } => CheckmarkItem {
                    label: name.clone(),
                    checked: active,
                    activate: Box::new(move |t: &mut Self| {
                        let _ = t.commands.send(Command::ActivateProfile(name.clone()));
                    }),
                    ..Default::default()
                }
                .into(),
                TrayItem::Separator => MenuItem::Separator,
                TrayItem::Open => StandardItem {
                    label: "Open MasterKeys".into(),
                    activate: Box::new(|t: &mut Self| (t.show)()),
                    ..Default::default()
                }
                .into(),
                TrayItem::Quit => StandardItem {
                    label: "Quit".into(),
                    activate: Box::new(|t: &mut Self| (t.quit)()),
                    ..Default::default()
                }
                .into(),
            })
            .collect()
    }
}

/// The running tray; `update` redraws it when the model changed.
pub struct TrayHandle {
    handle: ksni::blocking::Handle<Tray>,
    shown: TrayModel,
}

impl TrayHandle {
    pub fn update(&mut self, model: TrayModel) {
        if model != self.shown {
            self.shown = model.clone();
            self.handle.update(|t| t.model = model);
        }
    }
}

/// Put the icon in the system tray; `None` if the desktop has no tray.
pub fn spawn_tray(
    commands: UnboundedSender<Command>,
    show: impl Fn() + Send + Sync + 'static,
    quit: impl Fn() + Send + Sync + 'static,
) -> Option<TrayHandle> {
    use ksni::blocking::TrayMethods;
    let tray = Tray { model: TrayModel::default(), commands, show: Arc::new(show), quit: Arc::new(quit) };
    match tray.spawn() {
        Ok(handle) => Some(TrayHandle { handle, shown: TrayModel::default() }),
        Err(e) => {
            eprintln!("masterkeys: no system tray ({e})");
            None
        }
    }
}

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
    fn menu_without_daemon_has_no_profiles() {
        let m = TrayModel { profile: "B".into(), profiles: vec!["A".into()], connected: false };
        assert_eq!(
            menu_items(&m),
            vec![TrayItem::Header("MasterKeys — daemon not running".into()), TrayItem::Separator, TrayItem::Open, TrayItem::Quit]
        );
    }
}
