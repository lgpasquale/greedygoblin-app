#![cfg_attr(windows, windows_subsystem = "windows")]

mod api;
mod autostart;
mod cbor;
mod config;
mod day;
mod error;
mod game;
mod log;
mod paths;
mod state;
mod sync;
mod update;

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::config::{Config, Faction, Region, Ruleset};
use crate::error::SyncError;
use crate::sync::Syncer;

const SYNC_INTERVAL: Duration = Duration::from_secs(60 * 60);
#[cfg(windows)]
const UPDATE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Transient errors are shown only after failing for this long.
const TRANSIENT_GRACE: Duration = Duration::from_secs(3 * 60 * 60);

/// What the tray shows.
#[derive(Clone, Debug)]
enum Status {
    Busy,
    /// Sync is turned off from the menu.
    Disabled,
    Idle {
        last_scan: Option<i64>,
    },
    Error {
        message: String,
        last_scan: Option<i64>,
    },
}

enum UserEvent {
    Menu(MenuEvent),
    Status(Status),
    WowExePicked(PathBuf),
    #[cfg_attr(not(windows), allow(dead_code))]
    Restart,
}

/// Sent to the sync thread.
enum Command {
    Configure(Config),
}

fn main() {
    log::init(&paths::log_file());
    log!("greedygoblin-app {} started", env!("CARGO_PKG_VERSION"));

    let config_path = paths::config_file();
    let mut config = Config::load(&config_path).unwrap_or_else(|e| {
        log!("cannot read config, using defaults: {e}");
        Config::default()
    });
    if let Err(e) = autostart::apply(config.start_at_login) {
        log!("cannot set start at login: {e}");
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::Menu(event));
    }));

    let (commands, receiver) = mpsc::channel();
    let _ = commands.send(Command::Configure(config.clone()));
    let proxy = event_loop.create_proxy();
    thread::spawn(move || run_worker(receiver, proxy));

    let proxy = event_loop.create_proxy();
    let icons = Icons::load();
    let mut tray: Option<Tray> = None;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            // The tray must be created once the event loop runs.
            Event::NewEvents(StartCause::Init) => {
                let t = Tray::new(&icons);
                t.show_config(&config);
                tray = Some(t);
            }
            Event::UserEvent(UserEvent::Status(status)) => {
                if let Some(t) = &tray {
                    t.show_status(&icons, &status);
                }
            }
            Event::UserEvent(UserEvent::WowExePicked(path)) => {
                config.wow_exe = Some(path);
                apply_config(&config, &config_path, &commands, tray.as_ref());
            }
            Event::UserEvent(UserEvent::Restart) => {
                restart();
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::Menu(event)) => {
                let id = event.id.0.as_str();
                if id == "quit" {
                    *control_flow = ControlFlow::Exit;
                } else if id == "open-log" {
                    open_log();
                } else if id == "enabled" {
                    config.enabled = !config.enabled;
                    apply_config(&config, &config_path, &commands, tray.as_ref());
                } else if id == "start-at-login" {
                    config.start_at_login = !config.start_at_login;
                    if let Err(e) = autostart::apply(config.start_at_login) {
                        log!("cannot set start at login: {e}");
                    }
                    apply_config(&config, &config_path, &commands, tray.as_ref());
                } else if id == "wow" {
                    pick_wow_exe(&config, proxy.clone());
                } else if let Some(region) = id.strip_prefix("region:") {
                    if let Some(r) = Region::ALL.into_iter().find(|r| r.api_name() == region) {
                        config.region = r;
                        config.normalize();
                    }
                    apply_config(&config, &config_path, &commands, tray.as_ref());
                } else if let Some(ruleset) = id.strip_prefix("ruleset:") {
                    if let Some(r) = Ruleset::ALL.into_iter().find(|r| r.api_name() == ruleset) {
                        config.ruleset = r;
                        config.normalize();
                    }
                    apply_config(&config, &config_path, &commands, tray.as_ref());
                } else if let Some(faction) = id.strip_prefix("faction:") {
                    if let Some(f) = Faction::ALL.into_iter().find(|f| f.api_name() == faction) {
                        config.faction = f;
                    }
                    apply_config(&config, &config_path, &commands, tray.as_ref());
                }
            }
            _ => {}
        }
    });
}

fn apply_config(
    config: &Config,
    path: &std::path::Path,
    commands: &mpsc::Sender<Command>,
    tray: Option<&Tray>,
) {
    if let Some(tray) = tray {
        // Also undoes the toggle of a check item that was already checked.
        tray.show_config(config);
    }
    if let Err(e) = config.save(path) {
        log!("cannot save config: {e}");
    }
    let _ = commands.send(Command::Configure(config.clone()));
}

/// Shows the file dialog on its own thread, so the tray keeps working.
fn pick_wow_exe(config: &Config, proxy: EventLoopProxy<UserEvent>) {
    let start_dir = config
        .wow_exe
        .as_deref()
        .and_then(|p| p.parent())
        .map(|p| p.to_owned());
    thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Select Wow.exe")
            .add_filter("Wow.exe", &["exe"]);
        if let Some(dir) = start_dir.filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            let _ = proxy.send_event(UserEvent::WowExePicked(path));
        }
    });
}

fn open_log() {
    let path = paths::log_file();
    let program = if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program).arg(&path).spawn() {
        log!("cannot open {}: {e}", path.display());
    }
}

/// Starts the (updated) executable; the caller then quits.
fn restart() {
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(e) = std::process::Command::new(&exe).spawn() {
                log!("cannot restart {}: {e}", exe.display());
            }
        }
        Err(e) => log!("cannot restart: {e}"),
    }
}

/// The sync thread: syncs at start, when the config changes and every hour,
/// unless sync is disabled; on Windows it also checks for updates once a day.
fn run_worker(commands: mpsc::Receiver<Command>, proxy: EventLoopProxy<UserEvent>) {
    let syncer = Syncer::new(paths::state_dir(), day::scan_day_0());
    let send = |status: Status| {
        let _ = proxy.send_event(UserEvent::Status(status));
    };

    let mut config: Option<Config> = None;
    let mut next_sync = Instant::now();
    #[cfg(windows)]
    let mut next_update = Instant::now();
    let mut state = WorkerState {
        send: &send,
        failing_since: None,
        notified: None,
        last_scan: None,
    };

    loop {
        let timeout = next_sync.saturating_duration_since(Instant::now());
        match commands.recv_timeout(timeout) {
            Ok(Command::Configure(new)) => {
                if !config.as_ref().is_some_and(|c| c.same_sync(&new)) {
                    state.last_scan = syncer.last_scan_ts(&new);
                    state.failing_since = None;
                    next_sync = Instant::now();
                }
                config = Some(new);
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        next_sync = Instant::now() + SYNC_INTERVAL;
        let Some(config) = &config else {
            continue;
        };

        if config.enabled {
            sync(&syncer, config, &mut state);
        } else {
            send(Status::Disabled);
        }

        #[cfg(windows)]
        if Instant::now() >= next_update {
            next_update = Instant::now() + UPDATE_INTERVAL;
            match update::update() {
                Ok(true) => {
                    let _ = proxy.send_event(UserEvent::Restart);
                    return;
                }
                Ok(false) => {}
                Err(e) => log!("update failed: {e}"),
            }
        }
    }
}

/// What the sync thread remembers between syncs.
struct WorkerState<'a> {
    send: &'a dyn Fn(Status),
    failing_since: Option<Instant>,
    notified: Option<String>,
    last_scan: Option<i64>,
}

fn sync(syncer: &Syncer, config: &Config, state: &mut WorkerState) {
    let send = state.send;
    send(Status::Busy);
    let api = api::Api::new(&config.api_url);
    let now = chrono::Utc::now().timestamp();
    let last_scan = &mut state.last_scan;
    let status = match syncer.sync(&api, config, now) {
        Ok(ts) => {
            *last_scan = ts;
            state.failing_since = None;
            state.notified = None;
            Status::Idle {
                last_scan: *last_scan,
            }
        }
        Err(SyncError::Transient(message)) => {
            log!("sync failed: {message}");
            let since = *state.failing_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= TRANSIENT_GRACE {
                Status::Error {
                    message: "Cannot download the data".to_owned(),
                    last_scan: *last_scan,
                }
            } else {
                Status::Idle {
                    last_scan: *last_scan,
                }
            }
        }
        Err(SyncError::NeedsUser(message)) => {
            log!("sync failed: {message}");
            if state.notified.as_ref() != Some(&message) {
                notify(&message);
                state.notified = Some(message.clone());
            }
            Status::Error {
                message,
                last_scan: *last_scan,
            }
        }
    };
    send(status);
}

fn notify(message: &str) {
    if let Err(e) = notify_rust::Notification::new()
        .appname("GreedyGoblin")
        .summary("GreedyGoblin")
        .body(message)
        .show()
    {
        log!("cannot show notification: {e}");
    }
}

fn format_time(ts: i64) -> String {
    match Local.timestamp_opt(ts, 0).single() {
        Some(t) => t.format("%-d %b %H:%M").to_string(),
        None => ts.to_string(),
    }
}

struct Icons {
    normal: Icon,
    busy: Icon,
    error: Icon,
}

impl Icons {
    fn load() -> Self {
        fn icon(png: &[u8]) -> Icon {
            let image = image::load_from_memory(png)
                .expect("embedded icon is a PNG")
                .into_rgba8();
            let (width, height) = image.dimensions();
            Icon::from_rgba(image.into_raw(), width, height).expect("valid icon")
        }
        Self {
            normal: icon(include_bytes!("../assets/icon-normal.png")),
            busy: icon(include_bytes!("../assets/icon-busy.png")),
            error: icon(include_bytes!("../assets/icon-error.png")),
        }
    }
}

struct Tray {
    icon: TrayIcon,
    menu: Menu,
    wow: MenuItem,
    enabled: CheckMenuItem,
    start_at_login: CheckMenuItem,
    regions: Vec<(Region, CheckMenuItem)>,
    rulesets: Vec<(Ruleset, CheckMenuItem)>,
    factions: Vec<(Faction, CheckMenuItem)>,
    last_scan: MenuItem,
    error: MenuItem,
    error_shown: std::cell::Cell<bool>,
}

impl Tray {
    fn new(icons: &Icons) -> Self {
        let menu = Menu::new();
        let wow = MenuItem::with_id("wow", "WoW folder", true, None);
        let enabled = CheckMenuItem::with_id("enabled", "Sync enabled", true, true, None);
        let start_at_login =
            CheckMenuItem::with_id("start-at-login", "Start at login", true, true, None);

        let region_menu = Submenu::new("Region", true);
        let regions: Vec<_> = Region::ALL
            .into_iter()
            .map(|r| {
                let id = format!("region:{}", r.api_name());
                let label = r.api_name().to_uppercase();
                (r, CheckMenuItem::with_id(id, label, true, false, None))
            })
            .collect();
        let ruleset_menu = Submenu::new("Ruleset", true);
        let rulesets: Vec<_> = Ruleset::ALL
            .into_iter()
            .map(|r| {
                let id = format!("ruleset:{}", r.api_name());
                (
                    r,
                    CheckMenuItem::with_id(id, r.api_name(), true, false, None),
                )
            })
            .collect();
        let faction_menu = Submenu::new("Faction", true);
        let factions: Vec<_> = Faction::ALL
            .into_iter()
            .map(|f| {
                let id = format!("faction:{}", f.api_name());
                (
                    f,
                    CheckMenuItem::with_id(id, f.addon_name(), true, false, None),
                )
            })
            .collect();
        for (_, item) in &regions {
            let _ = region_menu.append(item);
        }
        for (_, item) in &rulesets {
            let _ = ruleset_menu.append(item);
        }
        for (_, item) in &factions {
            let _ = faction_menu.append(item);
        }

        let last_scan = MenuItem::new("Last scan downloaded: none", false, None);
        let error = MenuItem::new("", false, None);
        let _ = menu.append_items(&[
            &wow,
            &region_menu,
            &ruleset_menu,
            &faction_menu,
            &PredefinedMenuItem::separator(),
            &last_scan,
            &MenuItem::with_id("open-log", "Open log", true, None),
            &PredefinedMenuItem::separator(),
            &enabled,
            &start_at_login,
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id("quit", "Quit", true, None),
        ]);

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_tooltip("GreedyGoblin")
            .with_icon(icons.normal.clone())
            .build()
            .expect("cannot create the tray icon");

        Self {
            icon,
            menu,
            wow,
            enabled,
            start_at_login,
            regions,
            rulesets,
            factions,
            last_scan,
            error,
            error_shown: std::cell::Cell::new(false),
        }
    }

    fn show_config(&self, config: &Config) {
        let wow = match config.wow_exe.as_deref().and_then(|p| p.parent()) {
            Some(dir) => format!("WoW folder: {}", dir.display()),
            None => "WoW folder: not set".to_owned(),
        };
        self.wow.set_text(wow);
        self.enabled.set_checked(config.enabled);
        self.start_at_login.set_checked(config.start_at_login);
        for (region, item) in &self.regions {
            item.set_checked(*region == config.region);
        }
        for (ruleset, item) in &self.rulesets {
            item.set_checked(*ruleset == config.ruleset);
            item.set_enabled(config.region.has_ruleset(*ruleset));
        }
        for (faction, item) in &self.factions {
            item.set_checked(*faction == config.faction);
        }
    }

    fn show_status(&self, icons: &Icons, status: &Status) {
        let (icon, tooltip, error, last_scan) = match status {
            Status::Busy => (&icons.busy, "Syncing…".to_owned(), None, None),
            Status::Disabled => (&icons.normal, "Sync disabled".to_owned(), None, None),
            Status::Idle { last_scan } => (
                &icons.normal,
                match last_scan {
                    Some(ts) => format!("Last scan: {}", format_time(*ts)),
                    None => "No scan downloaded yet".to_owned(),
                },
                None,
                Some(*last_scan),
            ),
            Status::Error { message, last_scan } => (
                &icons.error,
                format!("Error: {message}"),
                Some(message.as_str()),
                Some(*last_scan),
            ),
        };
        let _ = self.icon.set_icon(Some(icon.clone()));
        let _ = self.icon.set_tooltip(Some(tooltip));
        if let Some(last_scan) = last_scan {
            self.last_scan.set_text(match last_scan {
                Some(ts) => format!("Last scan downloaded: {}", format_time(ts)),
                None => "Last scan downloaded: none".to_owned(),
            });
        }
        if matches!(status, Status::Busy) {
            return;
        }
        match error {
            Some(message) => {
                self.error.set_text(short(message));
                if !self.error_shown.replace(true) {
                    // Right under "Last scan downloaded".
                    let _ = self.menu.insert(&self.error, 6);
                }
            }
            None => {
                if self.error_shown.replace(false) {
                    let _ = self.menu.remove(&self.error);
                }
            }
        }
    }
}

/// The error, short enough for a menu line.
fn short(message: &str) -> String {
    const MAX: usize = 60;
    if message.chars().count() <= MAX {
        message.to_owned()
    } else {
        let cut: String = message.chars().take(MAX - 1).collect();
        format!("{cut}…")
    }
}
