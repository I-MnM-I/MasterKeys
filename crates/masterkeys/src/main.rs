//! MasterKeys: control panel for the SteelSeries Apex 3 (talks to masterkeysd).

slint::include_modules!();

use masterkeys::client::{self, Command, Event, Status};
use masterkeys::effects::{EffectKind, build, kind_of, parse_color_input};
use masterkeys::view::key_colors;
use masterkeys_core::layout::Layout;
use masterkeys_core::process;
use masterkeys_core::profile::ProfileSummary;
use masterkeys_core::render::Renderer;
use masterkeys_core::{LightingState, to_hex};
use slint::{Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;
use steelseries_gg::rgb::{Color, Effect};
use tokio::sync::mpsc::UnboundedSender;

const PRESETS: [Color; 12] = [
    Color::new(255, 255, 255),
    Color::new(255, 0, 0),
    Color::new(255, 128, 0),
    Color::new(255, 255, 0),
    Color::new(128, 255, 0),
    Color::new(0, 255, 0),
    Color::new(0, 255, 160),
    Color::new(0, 255, 255),
    Color::new(0, 128, 255),
    Color::new(0, 0, 255),
    Color::new(128, 0, 255),
    Color::new(255, 0, 255),
];
/// The effect list ends with an entry that is shown, not chosen, for per-zone colours.
const PER_ZONE_LABEL: &str = "Per-zone colours";

struct App {
    layout: Layout,
    renderer: Renderer,
    state: LightingState,
    pressed: HashSet<u16>,
    selected_zone: Option<u8>,
    slots: [Color; 2],
    active_slot: usize,
    profiles: Vec<ProfileSummary>,
    active_profile: String,
    selected_profile: Option<String>,
    commands: UnboundedSender<Command>,
}

impl App {
    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    fn selected(&self) -> Option<&ProfileSummary> {
        let name = self.selected_profile.as_ref()?;
        self.profiles.iter().find(|p| &p.name == name)
    }

    fn effect_from_panel(&self, ui: &MainWindow) -> Option<Effect> {
        let kind = *EffectKind::ALL.get(usize::try_from(ui.get_effect_index()).ok()?)?;
        Some(build(kind, self.slots[0], self.slots[1], ui.get_speed()))
    }
}

fn to_slint(c: Color) -> slint::Color {
    slint::Color::from_rgb_u8(c.r, c.g, c.b)
}

fn text_color(c: Color) -> slint::Color {
    let luma = 0.299 * f32::from(c.r) + 0.587 * f32::from(c.g) + 0.114 * f32::from(c.b);
    if luma > 140.0 { slint::Color::from_rgb_u8(0, 0, 0) } else { slint::Color::from_rgb_u8(255, 255, 255) }
}

fn set_slot(ui: &MainWindow, app: &mut App, slot: usize, color: Color) {
    app.slots[slot] = color;
    let hex: SharedString = to_hex(color).into();
    if slot == 0 {
        ui.set_slot_a_color(to_slint(color));
        ui.set_slot_a_hex(hex);
        ui.set_slot_a_valid(true);
    } else {
        ui.set_slot_b_color(to_slint(color));
        ui.set_slot_b_hex(hex);
        ui.set_slot_b_valid(true);
    }
}

/// Redraw the profile list and the selected profile's linked programs.
fn show_profiles(ui: &MainWindow, app: &mut App) {
    if app.selected().is_none() {
        app.selected_profile = Some(app.active_profile.clone()).filter(|n| app.profiles.iter().any(|p| &p.name == n));
    }
    let rows: Vec<SharedString> = app
        .profiles
        .iter()
        .map(|p| {
            let mark = if p.name == app.active_profile { "● " } else { "   " };
            let default = if p.is_default { "  (default)" } else { "" };
            format!("{mark}{}{default}", p.name).into()
        })
        .collect();
    ui.set_profile_rows(ModelRc::new(VecModel::from(rows)));
    let index = app.selected_profile.as_ref().and_then(|n| app.profiles.iter().position(|p| &p.name == n));
    ui.set_selected_profile(index.map_or(-1, |i| i as i32));
    let linked: Vec<SharedString> =
        app.selected().map(|p| p.executables.iter().map(|e| e.as_str().into()).collect()).unwrap_or_default();
    ui.set_linked_programs(ModelRc::new(VecModel::from(linked)));
}

/// Add a program to the selected profile's links.
fn link_program(app: &App, exe: &str) {
    let exe = exe.trim();
    let Some(profile) = app.selected() else { return };
    if exe.is_empty() || profile.executables.iter().any(|e| e == exe) {
        return;
    }
    let mut exes = profile.executables.clone();
    exes.push(exe.to_string());
    app.send(Command::SetExecutables(profile.name.clone(), exes));
}

fn apply_event(ui: &MainWindow, app: &mut App, event: Event) {
    match event {
        Event::Status(status) => {
            ui.set_status_text(status.text().into());
            ui.set_connected(status == Status::Connected);
            ui.set_daemon_running(status != Status::DaemonNotRunning);
        }
        Event::Profiles(list) => {
            app.profiles = list;
            show_profiles(ui, app);
        }
        Event::State(state, profile) => {
            if profile != app.active_profile {
                app.active_profile = profile;
                show_profiles(ui, app);
            }
            app.renderer.set_state(&state);
            ui.set_brightness(f32::from(state.brightness));
            let index = kind_of(&state.effect).map_or(EffectKind::ALL.len(), |k| {
                EffectKind::ALL.iter().position(|&x| x == k).unwrap_or(0)
            });
            ui.set_effect_index(index as i32);
            ui.set_show_speed(kind_of(&state.effect).is_some_and(EffectKind::has_speed));
            if let Effect::Breathing { speed, .. } | Effect::Spectrum { speed } | Effect::Wave { speed, .. } = state.effect
            {
                ui.set_speed(speed);
            }
            app.state = state;
            ui.set_error_text("".into());
        }
        Event::Key(code, true) => {
            app.pressed.insert(code);
        }
        Event::Key(code, false) => {
            app.pressed.remove(&code);
        }
        Event::Error(message) => ui.set_error_text(message.into()),
    }
}

fn refresh_keys(app: &mut App, model: &VecModel<KeyView>) {
    let frame = app.renderer.frame();
    let colors = key_colors(&app.layout, &frame, app.state.brightness);
    for (i, (key, color)) in app.layout.keys.iter().zip(colors).enumerate() {
        let view = KeyView {
            x: key.x,
            y: key.y,
            w: key.w,
            h: key.h,
            label: key.label.as_str().into(),
            color: to_slint(color),
            text_color: text_color(color),
            pressed: app.pressed.contains(&key.code),
            selected: app.selected_zone == Some(key.zone),
        };
        if model.row_data(i).as_ref() != Some(&view) {
            model.set_row_data(i, view);
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let (event_tx, event_rx) = mpsc::channel();
    let commands = client::spawn(move |event| {
        let _ = event_tx.send(event);
    });

    let layout = Layout::apex3_ansi();
    ui.set_board_width(layout.width());
    ui.set_board_height(layout.height());
    let model = Rc::new(VecModel::<KeyView>::default());
    for _ in &layout.keys {
        model.push(KeyView::default());
    }
    ui.set_keys(ModelRc::from(model.clone()));
    let mut labels: Vec<SharedString> = EffectKind::ALL.iter().map(|k| k.label().into()).collect();
    labels.push(PER_ZONE_LABEL.into());
    ui.set_effect_labels(ModelRc::new(VecModel::from(labels)));
    ui.set_presets(ModelRc::new(VecModel::from(PRESETS.iter().copied().map(to_slint).collect::<Vec<_>>())));

    let app = Rc::new(RefCell::new(App {
        layout,
        renderer: Renderer::new(),
        state: LightingState::default(),
        pressed: HashSet::new(),
        selected_zone: None,
        slots: [Color::WHITE, Color::BLUE],
        active_slot: 0,
        profiles: Vec::new(),
        active_profile: String::new(),
        selected_profile: None,
        commands,
    }));
    {
        let mut a = app.borrow_mut();
        set_slot(&ui, &mut a, 0, Color::WHITE);
        set_slot(&ui, &mut a, 1, Color::BLUE);
    }

    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_key_clicked(move |i| {
        let (Some(ui), Ok(i)) = (weak.upgrade(), usize::try_from(i)) else { return };
        let mut app = a.borrow_mut();
        let Some(zone) = app.layout.keys.get(i).map(|k| k.zone) else { return };
        app.selected_zone = Some(zone);
        ui.set_selected_zone(i32::from(zone));
        let color = app.renderer.current_colors()[usize::from(zone)];
        app.active_slot = 0;
        ui.set_active_slot(0);
        set_slot(&ui, &mut app, 0, color);
    });

    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_slot_selected(move |slot| {
        let Some(ui) = weak.upgrade() else { return };
        a.borrow_mut().active_slot = usize::from(slot == 1);
        ui.set_active_slot(slot);
    });

    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_hex_edited(move |slot, text| {
        let Some(ui) = weak.upgrade() else { return };
        let parsed = parse_color_input(&text);
        let mut app = a.borrow_mut();
        let slot = usize::from(slot == 1);
        if let Some(color) = parsed {
            app.slots[slot] = color;
        }
        if slot == 0 {
            ui.set_slot_a_valid(parsed.is_some());
            if let Some(c) = parsed {
                ui.set_slot_a_color(to_slint(c));
            }
        } else {
            ui.set_slot_b_valid(parsed.is_some());
            if let Some(c) = parsed {
                ui.set_slot_b_color(to_slint(c));
            }
        }
    });

    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_preset_clicked(move |i| {
        let (Some(ui), Some(&color)) = (weak.upgrade(), usize::try_from(i).ok().and_then(|i| PRESETS.get(i))) else {
            return;
        };
        let mut app = a.borrow_mut();
        let slot = app.active_slot;
        set_slot(&ui, &mut app, slot, color);
    });

    let a = app.clone();
    ui.on_apply_all(move || {
        let app = a.borrow();
        app.send(Command::SetAll(app.slots[0]));
    });

    let a = app.clone();
    ui.on_apply_zone(move || {
        let app = a.borrow();
        if let Some(zone) = app.selected_zone {
            app.send(Command::SetZone(zone, app.slots[0]));
        }
    });

    let a = app.clone();
    ui.on_brightness_released(move |v| a.borrow().send(Command::SetBrightness(v.round().clamp(0.0, 100.0) as u8)));

    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_effect_selected(move |_| {
        let Some(ui) = weak.upgrade() else { return };
        let app = a.borrow();
        if let Some(effect) = app.effect_from_panel(&ui) {
            ui.set_show_speed(kind_of(&effect).is_some_and(EffectKind::has_speed));
            app.send(Command::SetEffect(effect));
        }
    });

    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_speed_released(move |_| {
        let Some(ui) = weak.upgrade() else { return };
        let app = a.borrow();
        if let Some(effect) = app.effect_from_panel(&ui).filter(|e| kind_of(e).is_some_and(EffectKind::has_speed)) {
            app.send(Command::SetEffect(effect));
        }
    });

    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_profile_clicked(move |i| {
        let Some(ui) = weak.upgrade() else { return };
        let mut app = a.borrow_mut();
        let Some(name) = usize::try_from(i).ok().and_then(|i| app.profiles.get(i)).map(|p| p.name.clone()) else {
            return;
        };
        app.selected_profile = Some(name.clone());
        ui.set_profile_name(name.as_str().into());
        ui.set_show_running(false);
        show_profiles(&ui, &mut app);
        app.send(Command::ActivateProfile(name));
    });

    let a = app.clone();
    ui.on_profile_new(move |name| a.borrow().send(Command::CreateProfile(name.trim().to_string())));

    let a = app.clone();
    ui.on_profile_rename(move |new| {
        let mut app = a.borrow_mut();
        let Some(old) = app.selected().map(|p| p.name.clone()) else { return };
        let new = new.trim().to_string();
        app.selected_profile = Some(new.clone());
        app.send(Command::RenameProfile(old, new));
    });

    let a = app.clone();
    ui.on_profile_delete(move || {
        let app = a.borrow();
        if let Some(p) = app.selected() {
            app.send(Command::DeleteProfile(p.name.clone()));
        }
    });

    let a = app.clone();
    ui.on_profile_make_default(move || {
        let app = a.borrow();
        if let Some(p) = app.selected() {
            app.send(Command::SetDefault(p.name.clone()));
        }
    });

    let a = app.clone();
    ui.on_unlink_program(move |i| {
        let app = a.borrow();
        let (Some(p), Ok(i)) = (app.selected(), usize::try_from(i)) else { return };
        let mut exes = p.executables.clone();
        if i < exes.len() {
            exes.remove(i);
            app.send(Command::SetExecutables(p.name.clone(), exes));
        }
    });

    let weak = ui.as_weak();
    ui.on_link_running_requested(move || {
        let Some(ui) = weak.upgrade() else { return };
        let names: BTreeSet<String> = process::running().iter().filter_map(process::display_name).collect();
        let names: Vec<SharedString> = names.into_iter().map(SharedString::from).collect();
        ui.set_running_programs(ModelRc::new(VecModel::from(names)));
        ui.set_show_running(true);
    });

    let a = app.clone();
    ui.on_link_program(move |exe| link_program(&a.borrow(), &exe));

    let weak = ui.as_weak();
    ui.on_browse_program(move || {
        let weak = weak.clone();
        // kdialog blocks until the user picks, so run it off the UI thread.
        std::thread::spawn(move || {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
            let result = std::process::Command::new("kdialog").args(["--getopenfilename", &home]).output();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                match result {
                    Ok(out) if out.status.success() => {
                        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        if let Some(name) = std::path::Path::new(&path).file_name() {
                            ui.invoke_link_program(name.to_string_lossy().as_ref().into());
                        }
                    }
                    Ok(_) => {} // cancelled
                    Err(e) => ui.set_error_text(format!("cannot open a file picker (kdialog): {e}").into()),
                }
            });
        });
    });

    let a = app.clone();
    ui.on_start_daemon(move || a.borrow().send(Command::StartDaemon));

    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut app = app.borrow_mut();
        while let Ok(event) = event_rx.try_recv() {
            apply_event(&ui, &mut app, event);
        }
        refresh_keys(&mut app, &model);
    });

    ui.run()
}
