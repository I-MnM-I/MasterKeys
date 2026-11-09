//! MasterKeys: control panel for the SteelSeries Apex 3 (talks to masterkeysd).

slint::include_modules!();

use masterkeys::client::{self, Command, Event, Status};
use masterkeys::effects::{EffectKind, build, kind_of, parse_color_input};
use masterkeys::instance::claim_or_raise;
use masterkeys::tray::{CloseAction, TrayModel, on_close, spawn_tray};
use masterkeys::view::{
    LiveRecording, MODE_LABELS, binding_text, key_colors, key_name, marked, mode_from_index, mode_index, reselect,
};
use masterkeys_core::macro_text::{parse_text, to_text};
use masterkeys_core::InputStatus;
use masterkeys_core::bindings::Macro;
use std::collections::BTreeMap;
use masterkeys_core::layout::Layout;
use masterkeys_core::process;
use masterkeys_core::profile::ProfileSummary;
use masterkeys_core::render::Renderer;
use masterkeys_core::{LightingState, StateReport, to_hex};
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
    /// A rename sent to the daemon and not yet seen in a profile list: (old, new).
    pending_rename: Option<(String, String)>,
    remaps: BTreeMap<String, String>,
    macros: BTreeMap<String, Macro>,
    /// The key the Key panel is about (layout index).
    selected_key: Option<usize>,
    select_by_press: bool,
    recording: bool,
    /// The key being recorded for (fixed when Record was clicked) and what has been typed so far.
    recording_key: Option<String>,
    live: Option<LiveRecording>,
    /// The daemon is reachable (for the tray).
    daemon_up: bool,
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
    app.selected_profile =
        reselect(app.selected_profile.as_deref(), app.pending_rename.as_ref(), &app.profiles, &app.active_profile);
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

/// Show the selected key's bindings in the Key panel.
fn show_key_panel(ui: &MainWindow, app: &App) {
    let Some(key) = app.selected_key.and_then(|i| app.layout.keys.get(i)) else {
        ui.set_key_title("".into());
        return;
    };
    ui.set_key_title(format!("Key: {}", key.label).into());
    ui.set_key_binding(binding_text(key.code, &app.remaps, &app.macros).into());
    ui.set_key_remappable(key_name(key.code).is_some());
    ui.set_recording(app.recording);
}

/// Select a key (layout index): its zone for colours and the key for the Key panel.
fn select_key(ui: &MainWindow, app: &mut App, i: usize) {
    let Some(zone) = app.layout.keys.get(i).map(|k| k.zone) else { return };
    app.selected_key = Some(i);
    app.selected_zone = Some(zone);
    ui.set_selected_zone(i32::from(zone));
    let color = app.renderer.current_colors()[usize::from(zone)];
    app.active_slot = 0;
    ui.set_active_slot(0);
    set_slot(ui, app, 0, color);
    ui.set_remap_target("".into());
    load_macro(ui, app);
    show_key_panel(ui, app);
}

/// Put the selected key's macro (if any) into the macro box and repeat controls.
fn load_macro(ui: &MainWindow, app: &App) {
    let m = app
        .selected_key
        .and_then(|i| app.layout.keys.get(i))
        .and_then(|k| key_name(k.code))
        .and_then(|name| app.macros.get(name))
        .cloned()
        .unwrap_or_default();
    ui.set_macro_text(to_text(&m.steps).into());
    ui.set_macro_mode(mode_index(m.mode));
    ui.set_macro_count(m.count.to_string().into());
    ui.set_macro_gap(m.gap_ms.to_string().into());
    ui.set_macro_error("".into());
}

/// A macro from the panel's repeat controls and these steps, or what is wrong with them.
fn macro_from_panel(ui: &MainWindow, steps: Vec<masterkeys_core::bindings::MacroStep>) -> Result<Macro, String> {
    let mode = mode_from_index(ui.get_macro_mode());
    let count = ui.get_macro_count().trim().parse().map_err(|_| "times must be a whole number".to_string())?;
    let gap_ms = ui.get_macro_gap().trim().parse().map_err(|_| "gap must be a number of milliseconds".to_string())?;
    let m = Macro { steps, mode, count, gap_ms };
    masterkeys_core::bindings::validate_macro(&m)?;
    Ok(m)
}

fn apply_event(ui: &MainWindow, app: &mut App, event: Event) {
    match event {
        Event::Status(status) => {
            ui.set_status_text(status.text().into());
            ui.set_connected(status == Status::Connected);
            ui.set_daemon_running(status != Status::DaemonNotRunning);
            app.daemon_up = status != Status::DaemonNotRunning;
        }
        Event::Profiles(list) => {
            app.profiles = list;
            show_profiles(ui, app);
            app.pending_rename = None;
        }
        Event::State(report) => {
            let StateReport { state, profile, remaps, macros, input, gamesense, .. } = *report;
            ui.set_gamesense_enabled(gamesense.enabled);
            ui.set_gamesense_text(gamesense.game.map(|g| format!("GameSense: {g}")).unwrap_or_default().into());
            app.remaps = remaps;
            app.macros = macros;
            show_key_panel(ui, app);
            let (message, udev) = match input {
                InputStatus::Unavailable(m) => (m, true),
                InputStatus::Stopped(m) => (m, false),
                _ => (String::new(), false),
            };
            ui.set_input_message(message.into());
            ui.set_show_udev_command(udev);
            if profile != app.active_profile {
                app.active_profile = profile;
                show_profiles(ui, app);
                if !app.recording {
                    load_macro(ui, app);
                }
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
        Event::Recorded(m) => {
            ui.set_macro_text(to_text(&m.steps).into());
            if let Some(key) = app.recording_key.take() {
                match macro_from_panel(ui, m.steps) {
                    Ok(m) => {
                        ui.set_macro_error("".into());
                        app.send(Command::SetMacro(key, m));
                    }
                    Err(e) => ui.set_macro_error(e.into()),
                }
            }
        }
        Event::Key(code, true) => {
            app.pressed.insert(code);
            if let Some(live) = &mut app.live {
                live.feed(code, true, std::time::Instant::now());
                ui.set_macro_text(live.text().into());
            }
            if app.select_by_press && !app.recording {
                app.select_by_press = false;
                ui.set_select_by_press(false);
                if let Some(i) = app.layout.keys.iter().position(|k| k.code == code) {
                    select_key(ui, app, i);
                }
            }
        }
        Event::Key(code, false) => {
            app.pressed.remove(&code);
            if let Some(live) = &mut app.live {
                live.feed(code, false, std::time::Instant::now());
                ui.set_macro_text(live.text().into());
            }
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
            focused: app.selected_key == Some(i),
            marked: marked(key.code, &app.remaps, &app.macros),
        };
        if model.row_data(i).as_ref() != Some(&view) {
            model.set_row_data(i, view);
        }
    }
}

/// Show the window and start following key presses again.
fn show_window(ui: &MainWindow, commands: &UnboundedSender<Command>) {
    let _ = ui.show();
    let _ = commands.send(Command::KeysWanted(true));
}

fn main() -> Result<(), slint::PlatformError> {
    let start_hidden = std::env::args().skip(1).any(|a| a == "--tray");
    let Some(instance) = claim_or_raise() else {
        return Ok(()); // the running GUI was asked to show its window
    };
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
    ui.set_macro_modes(ModelRc::new(VecModel::from(MODE_LABELS.iter().map(|l| SharedString::from(*l)).collect::<Vec<_>>())));
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
        pending_rename: None,
        remaps: BTreeMap::new(),
        macros: BTreeMap::new(),
        selected_key: None,
        select_by_press: false,
        recording: false,
        recording_key: None,
        live: None,
        daemon_up: false,
        commands: commands.clone(),
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
        select_key(&ui, &mut a.borrow_mut(), i);
    });

    let a = app.clone();
    ui.on_remap_clicked(move |target| {
        let app = a.borrow();
        if let Some(name) = app.selected_key.and_then(|i| app.layout.keys.get(i)).and_then(|k| key_name(k.code)) {
            app.send(Command::SetRemap(name.into(), target.trim().to_string()));
        }
    });

    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_record_clicked(move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut app = a.borrow_mut();
        let Some(name) = app.selected_key.and_then(|i| app.layout.keys.get(i)).and_then(|k| key_name(k.code)) else {
            return;
        };
        if app.recording {
            app.live = None;
            app.send(Command::StopRecording);
        } else {
            app.recording_key = Some(name.into());
            app.live = Some(LiveRecording::start());
            ui.set_macro_text("".into());
            ui.set_macro_error("".into());
            app.send(Command::StartRecording);
        }
        app.recording = !app.recording;
        show_key_panel(&ui, &app);
    });

    let a = app.clone();
    ui.on_reset_key(move || {
        let app = a.borrow();
        let Some(name) = app.selected_key.and_then(|i| app.layout.keys.get(i)).and_then(|k| key_name(k.code)) else {
            return;
        };
        if app.remaps.contains_key(name) {
            app.send(Command::ClearRemap(name.into()));
        }
        if app.macros.contains_key(name) {
            app.send(Command::ClearMacro(name.into()));
        }
    });

    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_select_by_press_clicked(move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut app = a.borrow_mut();
        app.select_by_press = !app.select_by_press;
        ui.set_select_by_press(app.select_by_press);
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
        app.pending_rename = Some((old.clone(), new.clone()));
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

    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_save_macro(move || {
        let Some(ui) = weak.upgrade() else { return };
        let app = a.borrow();
        let Some(name) = app.selected_key.and_then(|i| app.layout.keys.get(i)).and_then(|k| key_name(k.code)) else {
            return;
        };
        match parse_text(&ui.get_macro_text()).and_then(|steps| macro_from_panel(&ui, steps)) {
            Ok(m) => {
                ui.set_macro_error("".into());
                app.send(Command::SetMacro(name.into(), m));
            }
            Err(e) => ui.set_macro_error(e.into()),
        }
    });

    let a = app.clone();
    ui.on_gamesense_toggled(move |on| a.borrow().send(Command::SetGameSense(on)));

    let a = app.clone();
    ui.on_start_daemon(move || a.borrow().send(Command::StartDaemon));

    let raise = {
        let weak = ui.as_weak();
        let commands = commands.clone();
        move || {
            let (weak, commands) = (weak.clone(), commands.clone());
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    show_window(&ui, &commands);
                }
            });
        }
    };
    instance.on_show(raise.clone());
    let mut tray = spawn_tray(commands.clone(), raise, || {
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    });
    let tray_ok = tray.is_some();
    // Closing the window hides it when the tray (or a second launch) can bring it back; else quit.
    let close_commands = commands.clone();
    ui.window().on_close_requested(move || match on_close(tray_ok) {
        CloseAction::HideToTray => {
            let _ = close_commands.send(Command::KeysWanted(false));
            slint::CloseRequestResponse::HideWindow
        }
        CloseAction::Quit => {
            let _ = slint::quit_event_loop();
            slint::CloseRequestResponse::HideWindow
        }
    });

    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut app = app.borrow_mut();
        while let Ok(event) = event_rx.try_recv() {
            apply_event(&ui, &mut app, event);
        }
        refresh_keys(&mut app, &model);
        if let Some(tray) = &mut tray {
            tray.update(TrayModel {
                profile: app.active_profile.clone(),
                profiles: app.profiles.iter().map(|p| p.name.clone()).collect(),
                connected: app.daemon_up,
            });
        }
    });

    // Without a tray there would be no way back to a hidden window, so `--tray` then shows it anyway.
    if start_hidden && tray_ok {
        let _ = commands.send(Command::KeysWanted(false));
    } else {
        show_window(&ui, &commands);
    }
    slint::run_event_loop_until_quit()
}
