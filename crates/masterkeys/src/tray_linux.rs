//! The tray on Linux: a StatusNotifierItem through ksni.

use super::{TrayItem, TrayModel, menu_items};
use crate::client::Command;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

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

