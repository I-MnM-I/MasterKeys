//! The tray on Windows: a notification-area icon through tray-icon, on the GUI's thread.

use super::{TrayItem, TrayModel, menu_items};
use crate::client::Command;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// What a menu entry does.
enum Act {
    Profile(String),
    Open,
    Quit,
}

struct Inner {
    icon: TrayIcon,
    acts: HashMap<MenuId, Act>,
    shown: TrayModel,
}

/// The running tray; `update` rebuilds the menu when the model changed.
pub struct TrayHandle {
    inner: Rc<RefCell<Inner>>,
    _poll: slint::Timer,
}

impl TrayHandle {
    pub fn update(&mut self, model: TrayModel) {
        let mut inner = self.inner.borrow_mut();
        if model != inner.shown {
            let (menu, acts) = build_menu(&model);
            inner.icon.set_menu(Some(Box::new(menu)));
            inner.icon.set_tooltip(Some(if model.connected { format!("MasterKeys — {}", model.profile) } else { "MasterKeys".into() })).ok();
            inner.acts = acts;
            inner.shown = model;
        }
    }
}

fn build_menu(model: &TrayModel) -> (Menu, HashMap<MenuId, Act>) {
    let menu = Menu::new();
    let mut acts = HashMap::new();
    for item in menu_items(model) {
        let _ = match item {
            TrayItem::Header(label) => menu.append(&MenuItem::new(label, false, None)),
            TrayItem::Profile { name, active } => {
                let entry = CheckMenuItem::new(&name, true, active, None);
                acts.insert(entry.id().clone(), Act::Profile(name));
                menu.append(&entry)
            }
            TrayItem::Separator => menu.append(&PredefinedMenuItem::separator()),
            TrayItem::Open => {
                let entry = MenuItem::new("Open MasterKeys", true, None);
                acts.insert(entry.id().clone(), Act::Open);
                menu.append(&entry)
            }
            TrayItem::Quit => {
                let entry = MenuItem::new("Quit", true, None);
                acts.insert(entry.id().clone(), Act::Quit);
                menu.append(&entry)
            }
        };
    }
    (menu, acts)
}

/// A small keyboard: a dark rounded body with rows of light keys.
fn icon() -> Option<Icon> {
    const N: u32 = 32;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let body = (2..30).contains(&x) && (8..25).contains(&y);
            let key = body && (x - 4) % 4 < 3 && (y - 10) % 5 < 3 && (4..28).contains(&x) && (10..23).contains(&y);
            let px: [u8; 4] = if key {
                [0xE8, 0xE8, 0xF0, 0xFF]
            } else if body {
                [0x2A, 0x2D, 0x3A, 0xFF]
            } else {
                [0, 0, 0, 0]
            };
            rgba.extend_from_slice(&px);
        }
    }
    Icon::from_rgba(rgba, N, N).ok()
}

/// Put the icon in the notification area; `None` if Windows refused it.
pub fn spawn_tray(
    commands: UnboundedSender<Command>,
    show: impl Fn() + Send + Sync + 'static,
    quit: impl Fn() + Send + Sync + 'static,
) -> Option<TrayHandle> {
    let model = TrayModel::default();
    let (menu, acts) = build_menu(&model);
    let mut builder = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("MasterKeys").with_menu_on_left_click(false);
    if let Some(icon) = icon() {
        builder = builder.with_icon(icon);
    }
    let icon = match builder.build() {
        Ok(icon) => icon,
        Err(e) => {
            eprintln!("masterkeys: no tray icon ({e})");
            return None;
        }
    };
    let inner = Rc::new(RefCell::new(Inner { icon, acts, shown: model }));
    let poll = slint::Timer::default();
    let state = inner.clone();
    poll.start(slint::TimerMode::Repeated, Duration::from_millis(100), move || {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            match state.borrow().acts.get(event.id()) {
                Some(Act::Profile(name)) => drop(commands.send(Command::ActivateProfile(name.clone()))),
                Some(Act::Open) => show(),
                Some(Act::Quit) => quit(),
                None => {}
            }
        }
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                show();
            }
        }
    });
    Some(TrayHandle { inner, _poll: poll })
}
