//! The main window.
//!
//! The window owns the database while it is idle and hands it to a worker thread for an
//! operation (loading the cache, updating DATs, scanning); the thread gives it back with the
//! result. While it is away the tree and lists stay as they were and the buttons are off, so the
//! UI thread never waits for the engine and never reads a database that is being changed.
//! Everything the engine does is a call into [`crate::engine`].

mod grids;
mod icons;
mod key;
mod painted;
mod pickers;
mod settings;
mod tree_view;
mod virtual_list;

use std::cell::RefCell;
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gtk4::prelude::*;
use gtk4::{gdk, glib};
use relm4::prelude::*;
use rv_core::progress::{CancelFlag, ProgressEvent, ProgressSink};
use rv_core::{Database, ScanLevel, Settings};
use rv_scan::db::ScanTarget;

use crate::engine::{self, Outcome, Task};
use crate::model::{self, DirNode, GameFilter};
use crate::prefs::{self, Prefs};
use grids::{GameEvent, GameGrid, RomGrid};
use tree_view::{TreeEvent, TreeView};

/// What the command line asked for. The last six are for automation (screenshots, the smoke
/// test); a person never needs them.
#[derive(Clone, Debug)]
pub struct Args {
    pub config: PathBuf,
    pub prefs: PathBuf,
    /// The online settings (the DatVault key and the rest).
    pub online: PathBuf,
    /// Select this tree row (`RomVault/Console/NES`) once the cache is loaded.
    pub select: Option<String>,
    /// Select this game in the list (by name).
    pub game: Option<String>,
    /// Open this dialog at start: `settings`, `settings-dirs`, `settings-rules`, `key` or `picker` (the folder picker).
    pub open: Option<String>,
    /// Start this operation once the cache is loaded: `update`, `scan`.
    pub start: Option<String>,
    /// Load, report what is on screen, and quit.
    pub smoke: bool,
    pub started: Instant,
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
        let mut out = Args {
            config: PathBuf::from(engine::DEFAULT_CONFIG),
            prefs: prefs::default_path(),
            online: rv_online::config::default_path(),
            select: None,
            game: None,
            open: None,
            start: None,
            smoke: false,
            started: Instant::now(),
        };
        let mut it = args.into_iter().skip(1);
        while let Some(a) = it.next() {
            let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
            match a.as_str() {
                "--config" => out.config = PathBuf::from(value("--config")?),
                "--prefs" => out.prefs = PathBuf::from(value("--prefs")?),
                "--online-config" => out.online = PathBuf::from(value("--online-config")?),
                "--select" => out.select = Some(value("--select")?),
                "--game" => out.game = Some(value("--game")?),
                "--open" => out.open = Some(value("--open")?),
                "--start" => out.start = Some(value("--start")?),
                "--smoke" => out.smoke = true,
                "-h" | "--help" => return Err(USAGE.to_owned()),
                other => return Err(format!("unknown argument {other}\n{USAGE}")),
            }
        }
        Ok(out)
    }
}

const USAGE: &str = "usage: rv-gtk [--config RomVault3cfg.xml] [--prefs rv-gtk.conf]\n\
\n\
The directory of the config file is where relative paths in it (DatRoot, the cache, the ROM\n\
directories) are taken from, as with ROMVault.";

/// Wraps a message payload that has no useful `Debug`.
pub struct Opaque<T>(pub T);

impl<T> fmt::Debug for Opaque<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("..")
    }
}

/// What a worker thread gives back.
pub struct Finished {
    pub db: Database,
    pub tree: Vec<Arc<DirNode>>,
    pub result: Result<Option<Outcome>, String>,
    pub took: Duration,
    /// Set when the load fell back to the cache's backup file.
    pub notice: Option<String>,
}

#[derive(Debug)]
pub enum Msg {
    /// Read the settings file and the cache again.
    Reload,
    Loaded(Opaque<Result<Finished, String>>),
    RunTask(Task),
    /// The Scan button: the level is the dropdown's, the target the ticked rows.
    ScanSelected,
    Progress(Opaque<ProgressEvent>),
    TaskDone(Opaque<Finished>),
    Cancel,
    Tree(Opaque<TreeEvent>),
    Game(Opaque<GameEvent>),
    FilterChanged(Opaque<GameFilter>),
    ScanLevelChanged(ScanLevel),
    /// The settings dialog, on the page with this number.
    OpenSettings(u32),
    OpenKey,
    /// The Online menu.
    CheckKey,
    KeyChecked(String),
    DatVaultUpdate,
    UpdateMia(bool),
    SettingsApplied(Opaque<(Settings, Prefs)>),
    CloseRequested,
}

/// What the window keeps while an operation runs.
struct Running {
    cancel: CancelFlag,
}

struct Widgets {
    window: gtk4::Window,
    tree: TreeView,
    games: Rc<GameGrid>,
    roms: RomGrid,
    info_title: gtk4::Label,
    info_fields: gtk4::Label,
    info_counts: gtk4::Label,
    banner: gtk4::Revealer,
    banner_text: gtk4::Label,
    status: gtk4::Label,
    progress: Progress,
    update: gtk4::Button,
    update_all: gtk4::Button,
    scan: gtk4::Button,
    scan_level: gtk4::DropDown,
    find_fixes: gtk4::Button,
    fix: gtk4::Button,
    online: gtk4::MenuButton,
    settings: gtk4::Button,
}

/// The panel at the bottom while something runs.
struct Progress {
    revealer: gtk4::Revealer,
    title: gtk4::Label,
    item: gtk4::Label,
    detail: gtk4::Label,
    overall: gtk4::ProgressBar,
    sub: gtk4::ProgressBar,
    cancel: gtk4::Button,
    errors: gtk4::TextBuffer,
    error_expander: gtk4::Expander,
    error_count: std::cell::Cell<usize>,
    /// Which run the pulsing of the bar belongs to; a new run stops the old timer.
    pulse: Rc<std::cell::Cell<u64>>,
}

pub struct App {
    w: Widgets,
    args: Args,
    prefs: Prefs,
    config_path: PathBuf,
    settings: Settings,
    db: Option<Database>,
    tree_rows: Vec<Arc<DirNode>>,
    selected: Option<Vec<String>>,
    task: Option<Running>,
    closing: bool,
    /// A tick box changed since the cache was saved.
    dirty: bool,
    first_load: bool,
}

fn set_text_bold(label: &gtk4::Label, text: &str) {
    label.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(text)));
}

/// Forwards progress to the window, at most about twenty times a second per kind of message, so
/// that a scan over a million files does not queue a million redraws.
struct Throttle {
    tx: relm4::Sender<Msg>,
    last: RefCell<[Option<Instant>; 4]>,
}

impl ProgressSink for Throttle {
    fn report(&self, event: ProgressEvent) {
        let slot = match &event {
            ProgressEvent::Item(_) => Some(0),
            ProgressEvent::Detail(_) => Some(1),
            ProgressEvent::Overall { .. } => Some(2),
            ProgressEvent::Sub { .. } => Some(3),
            ProgressEvent::Text(_) | ProgressEvent::Error { .. } => None,
        };
        if let Some(i) = slot {
            let mut last = self.last.borrow_mut();
            let now = Instant::now();
            if last[i].is_some_and(|t| now.duration_since(t) < Duration::from_millis(50)) {
                return;
            }
            last[i] = Some(now);
        }
        let _ = self.tx.send(Msg::Progress(Opaque(event)));
    }
}

fn worker(f: impl FnOnce() + Send + 'static) {
    // Deep trees recurse; give the thread more than the 2 MiB default.
    let _ = std::thread::Builder::new()
        .name("rv-engine".to_owned())
        .stack_size(32 * 1024 * 1024)
        .spawn(f);
}

fn count_rows(rows: &[Arc<DirNode>]) -> usize {
    rows.iter().map(|r| 1 + count_rows(&r.children)).sum()
}

fn set_bar(bar: &gtk4::ProgressBar, done: u64, total: u64) {
    if total == 0 {
        bar.set_fraction(0.0);
        bar.set_text(None);
    } else {
        bar.set_fraction((done as f64 / total as f64).clamp(0.0, 1.0));
        bar.set_text(Some(&format!("{done} / {total}")));
    }
}

impl App {
    fn busy(&self) -> bool {
        self.task.is_some()
    }

    /// The online settings as they are in the file now, so that a key typed in the settings
    /// dialog counts at once. Without a key there is nothing to ask the service. `needs_db` for
    /// the operations that change the database.
    fn online_config(&self, needs_db: bool) -> Result<rv_online::config::OnlineConfig, String> {
        if self.busy() || (needs_db && self.db.is_none()) {
            return Err("Wait until the current operation is finished.".to_owned());
        }
        let cfg =
            rv_online::config::OnlineConfig::load(&self.args.online).map_err(|e| e.to_string())?;
        if !cfg.has_key() {
            return Err(
                "A DatVault key is required: enter it in Settings, on the Online page.".to_owned(),
            );
        }
        Ok(cfg)
    }

    fn status(&self, text: &str) {
        self.w.status.set_text(text);
    }

    fn show_banner(&self, text: Option<&str>) {
        match text {
            Some(t) => {
                self.w.banner_text.set_text(t);
                self.w.banner.set_reveal_child(true);
            }
            None => self.w.banner.set_reveal_child(false),
        }
    }

    /// The buttons that start an operation work only with a database in hand and nothing running.
    fn update_buttons(&self) {
        let ready = self.db.is_some() && !self.busy();
        for b in [&self.w.update, &self.w.update_all, &self.w.scan] {
            b.set_sensitive(ready);
        }
        self.w.scan_level.set_sensitive(ready);
        self.w.online.set_sensitive(ready);
        // Settings has to work when the cache could not be loaded: that is how it gets fixed.
        self.w.settings.set_sensitive(!self.busy());
        // Find Fixes and Fix are rv-fix, Phase 5b: present, off, with the reason.
        for b in [&self.w.find_fixes, &self.w.fix] {
            b.set_sensitive(false);
            b.set_tooltip_text(Some(engine::FIX_PENDING));
        }
    }

    fn find_row(&self, path: &[String]) -> Option<Arc<DirNode>> {
        let (first, rest) = path.split_first()?;
        let mut cur = self.tree_rows.iter().find(|r| &r.name == first)?.clone();
        for name in rest {
            let next = cur.children.iter().find(|r| &r.name == name)?.clone();
            cur = next;
        }
        Some(cur)
    }

    // ---- loading and running -------------------------------------------------------------

    fn reload(&mut self, sender: &ComponentSender<App>) {
        self.show_banner(None);
        match engine::read_settings(&self.config_path) {
            Ok(s) => self.settings = s,
            Err(e) => {
                self.show_banner(Some(&e.to_string()));
                self.db = None;
                self.update_buttons();
                return;
            }
        }
        // The guard comes before any file is opened.
        if let Err(e) = engine::check_paths(&self.settings, self.prefs.allow_protected_paths) {
            self.db = None;
            self.show_banner(Some(&format!(
                "{e}\nChange the paths in Settings. (The guard can be lifted there on purpose.)"
            )));
            self.status("Not loaded: protected path");
            self.update_buttons();
            return;
        }
        self.status("Loading the cache...");
        self.start_progress("Loading the cache", true);
        let settings = self.settings.clone();
        let allow = self.prefs.allow_protected_paths;
        let tx = sender.input_sender().clone();
        worker(move || {
            let result = engine::open_cache(&settings, allow)
                .map(|o| Finished {
                    tree: model::build_tree(&o.db.root, settings.show_new_mia),
                    notice: o.fallback_notice(),
                    db: o.db,
                    result: Ok(None),
                    took: o.took,
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::Loaded(Opaque(result)));
        });
        self.task = Some(Running {
            cancel: CancelFlag::new(),
        });
        self.update_buttons();
    }

    fn run_task(&mut self, task: Task, sender: &ComponentSender<App>) {
        if self.busy() {
            return;
        }
        let Some(mut db) = self.db.take() else { return };
        self.sync_tree_state(&mut db);
        self.dirty = false;
        let cancel = CancelFlag::new();
        let settings = self.settings.clone();
        let allow = self.prefs.allow_protected_paths;
        let tx = sender.input_sender().clone();
        let flag = cancel.clone();
        self.task = Some(Running { cancel });
        self.start_progress(&task.title(), false);
        self.update_buttons();
        worker(move || {
            let start = Instant::now();
            let throttle = Throttle {
                tx: tx.clone(),
                last: RefCell::new([None; 4]),
            };
            let result = engine::run_task(&mut db, &settings, &task, allow, &throttle, &flag)
                .map(Some)
                .map_err(|e| e.to_string());
            let tree = model::build_tree(&db.root, settings.show_new_mia);
            let _ = tx.send(Msg::TaskDone(Opaque(Finished {
                db,
                tree,
                result,
                took: start.elapsed(),
                notice: None,
            })));
        });
    }

    /// Write what the tree widget knows about open folders into the database.
    fn sync_tree_state(&self, db: &mut Database) -> bool {
        let mut changed = false;
        for (path, open) in self.w.tree.open_state() {
            if model::dir_at(&db.root, &path)
                .and_then(|n| n.tree)
                .is_some_and(|t| t.expanded != open)
            {
                model::set_expanded(&mut db.root, &path, open);
                changed = true;
            }
        }
        changed
    }

    fn start_progress(&self, title: &str, indeterminate: bool) {
        let p = &self.w.progress;
        set_text_bold(&p.title, title);
        p.item.set_text("");
        p.detail.set_text("");
        p.overall.set_fraction(0.0);
        p.overall.set_text(None);
        p.sub.set_fraction(0.0);
        p.sub.set_text(None);
        p.sub.set_visible(!indeterminate);
        p.cancel.set_sensitive(!indeterminate);
        p.errors.set_text("");
        p.error_count.set(0);
        p.error_expander.set_visible(false);
        p.revealer.set_reveal_child(true);
        let run = p.pulse.get() + 1;
        p.pulse.set(run);
        if indeterminate {
            let (bar, current) = (p.overall.clone(), p.pulse.clone());
            glib::timeout_add_local(Duration::from_millis(120), move || {
                if current.get() == run {
                    bar.pulse();
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            });
        }
    }

    fn progress(&self, event: ProgressEvent) {
        let p = &self.w.progress;
        match event {
            ProgressEvent::Text(t) => set_text_bold(&p.title, &t),
            ProgressEvent::Item(t) => p.item.set_text(&t),
            ProgressEvent::Detail(t) => p.detail.set_text(&t),
            ProgressEvent::Overall { done, total } => set_bar(&p.overall, done, total),
            ProgressEvent::Sub { done, total } => set_bar(&p.sub, done, total),
            ProgressEvent::Error { file, message } => {
                let n = p.error_count.get() + 1;
                p.error_count.set(n);
                // Keep the list a size a text view can show.
                if n <= 2000 {
                    let mut end = p.errors.end_iter();
                    p.errors.insert(&mut end, &format!("{file}: {message}\n"));
                }
                p.error_expander.set_label(Some(&format!("{n} problems")));
                p.error_expander.set_visible(true);
            }
        }
    }

    // ---- showing ----------------------------------------------------------------------

    fn show_tree(&mut self, rows: Vec<Arc<DirNode>>) {
        self.tree_rows = rows;
        let keep = self.selected.clone();
        self.w.tree.set_rows(&self.tree_rows, keep.as_deref());
        if keep.as_ref().is_some_and(|p| self.find_row(p).is_none()) {
            self.selected = None;
        }
        self.refresh_games(None);
    }

    fn refresh_games(&mut self, keep: Option<usize>) {
        let (Some(db), Some(path)) = (&self.db, &self.selected) else {
            self.w.games.set_list(Default::default(), None);
            self.w.roms.clear();
            self.w.info_title.set_text("");
            self.w.info_fields.set_text("");
            self.w.info_counts.set_text("");
            return;
        };
        let Some(dir) = model::dir_at(&db.root, path) else {
            self.w.games.set_list(Default::default(), None);
            self.w.roms.clear();
            return;
        };
        let show_new_mia = self.settings.show_new_mia;
        let t = Instant::now();
        let list = model::game_list(dir, &self.prefs.filter, show_new_mia);
        let (n, build) = (list.rows.len(), t.elapsed());
        self.w.games.set_list(list, keep);
        eprintln!(
            "rv-gtk: game list of {}: {n} games, built in {:.1} ms, shown in {:.1} ms",
            path.join("/"),
            build.as_secs_f64() * 1000.0,
            (t.elapsed() - build).as_secs_f64() * 1000.0
        );
        let info = model::dir_info(&db.root, path, &dir.dir_status(show_new_mia));
        set_text_bold(&self.w.info_title, &info.title);
        let fields: Vec<String> = info
            .fields
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();
        self.w.info_fields.set_text(&fields.join("\n"));
        self.w.info_counts.set_text(&info.counts);
        self.refresh_roms();
    }

    fn refresh_roms(&self) {
        let (Some(db), Some(path)) = (&self.db, &self.selected) else {
            self.w.roms.clear();
            return;
        };
        let Some(dir) = model::dir_at(&db.root, path) else {
            self.w.roms.clear();
            return;
        };
        let source = self
            .w
            .games
            .selected_index()
            .and_then(|i| dir.children.get(i))
            .unwrap_or(dir);
        self.w.roms.set_list(model::rom_list(
            source,
            self.prefs.filter.merged,
            self.settings.show_new_mia,
        ));
    }

    fn report_load_time(&self, took: Duration) {
        self.status(&format!(
            "Cache loaded in {:.2} s; {} rows in the tree",
            took.as_secs_f64(),
            count_rows(&self.tree_rows)
        ));
        let started = self.args.started;
        eprintln!(
            "rv-gtk: cache loaded in {} ms ({} ms after start); waiting for the first frame",
            took.as_millis(),
            started.elapsed().as_millis()
        );
        once_after_paint(&self.w.window, move || {
            eprintln!(
                "rv-gtk: first frame with the tree at {} ms after start",
                started.elapsed().as_millis()
            );
        });
    }

    fn follow_args_after_load(&mut self, sender: &ComponentSender<App>) {
        if !self.first_load {
            return;
        }
        self.first_load = false;
        if let Some(sel) = self.args.select.clone() {
            self.selected = Some(
                sel.split('/')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
            );
            let rows = self.tree_rows.clone();
            self.show_tree(rows);
        }
        if let Some(game) = self.args.game.clone() {
            // The selection event is queued behind this message; show the ROMs now.
            self.w.games.select_name(&game);
            self.refresh_roms();
        }
        match self.args.open.as_deref() {
            Some("settings") => sender.input(Msg::OpenSettings(0)),
            Some("settings-dirs") => sender.input(Msg::OpenSettings(1)),
            Some("settings-rules") => sender.input(Msg::OpenSettings(2)),
            Some("settings-online") => sender.input(Msg::OpenSettings(4)),
            Some("key") => sender.input(Msg::OpenKey),
            Some("picker") => {
                // The same call the Browse buttons make.
                let start = self.settings.dat_root.clone();
                pickers::pick_folder(&self.w.window, "Choose the DAT root", &start, |p| {
                    eprintln!("rv-gtk: the picker chose {}", p.display());
                });
            }
            _ => {}
        }
        match self.args.start.as_deref() {
            Some("update") => sender.input(Msg::RunTask(Task::UpdateDats { all: false })),
            Some("scan") => sender.input(Msg::ScanSelected),
            _ => {}
        }
        if self.args.smoke {
            self.smoke_report();
        }
    }

    /// `--smoke`: say what the window shows, then quit. The exit status is the test's verdict.
    fn smoke_report(&self) {
        println!(
            "smoke: tree_rows={} selected={:?} games={} roms={} ms={}",
            count_rows(&self.tree_rows),
            self.selected.as_ref().map(|p| p.join("/")),
            self.w.games.count(),
            self.w.roms.count(),
            self.args.started.elapsed().as_millis()
        );
        let w = self.w.window.clone();
        // One frame first, so that the lists have been drawn at least once.
        once_after_paint(&self.w.window, move || {
            if let Some(app) = w.application() {
                app.quit();
            } else {
                w.close();
            }
        });
    }

    fn save_prefs(&self) {
        if let Err(e) = self.prefs.save(&self.args.prefs) {
            eprintln!("rv-gtk: preferences not saved: {e}");
        }
    }

    /// Remember the window and the tree state, save the cache, and go. The cache carries the tick
    /// boxes and the open folders, so it is saved whenever the window closes.
    fn finish_close(&mut self) {
        let window = self.w.window.clone();
        self.prefs.width = window.width().max(400);
        self.prefs.height = window.height().max(300);
        self.save_prefs();
        if let Some(mut db) = self.db.take() {
            let changed = self.sync_tree_state(&mut db);
            if (changed || self.dirty) && !self.args.smoke {
                if let Err(e) =
                    engine::save_cache(&self.settings, &mut db, self.prefs.allow_protected_paths)
                {
                    eprintln!("rv-gtk: cache not saved: {e}");
                }
            }
            self.db = Some(db);
        }
        window.destroy();
    }
}

/// Run `f` once, after the next frame of `window` has been painted.
fn once_after_paint(window: &gtk4::Window, f: impl Fn() + 'static) {
    let Some(clock) = window.frame_clock() else {
        f();
        return;
    };
    let id: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
    let id2 = id.clone();
    let handler = clock.connect_after_paint(move |c| {
        if let Some(h) = id2.borrow_mut().take() {
            c.disconnect(h);
            f();
        }
    });
    *id.borrow_mut() = Some(handler);
}

impl SimpleComponent for App {
    type Init = Args;
    type Input = Msg;
    type Output = ();
    type Root = gtk4::Window;
    type Widgets = ();

    fn init_root() -> Self::Root {
        gtk4::Window::builder().title("rv-rs").build()
    }

    fn init(args: Args, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let prefs = Prefs::load(&args.prefs);
        root.set_default_size(prefs.width, prefs.height);
        // Below this the filter row and the lists are clipped with no way to reach them.
        root.set_size_request(MIN_WINDOW.0, MIN_WINDOW.1);
        install_css();

        // The toolbar.
        let button = |label: &str, tip: &str| {
            let b = gtk4::Button::with_label(label);
            b.set_tooltip_text(Some(tip));
            b
        };
        let update = button(
            "Update DATs",
            "Read new and changed DATs under DatRoot into the database",
        );
        let update_all = button("All", "Read every DAT again, changed or not");
        let scan = button(
            "Scan ROMs",
            "Scan the ticked directories at the chosen level",
        );
        let scan_level =
            gtk4::DropDown::from_strings(&["Quick (headers only)", "Normal", "Full (re-scan)"]);
        scan_level.set_selected(match prefs.scan_level {
            ScanLevel::Level1 => 0,
            ScanLevel::Level2 => 1,
            ScanLevel::Level3 => 2,
        });
        let find_fixes = button("Find Fixes", engine::FIX_PENDING);
        let fix = button("Fix Files", engine::FIX_PENDING);
        let settings_btn = button("Settings", "Directories, DAT rules and the cache file");
        let key_btn = button("Key", "What the colours and symbols mean");

        let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        toolbar.set_margin_start(6);
        toolbar.set_margin_end(6);
        toolbar.set_margin_top(6);
        let updates = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        updates.add_css_class("linked");
        updates.append(&update);
        updates.append(&update_all);
        let scans = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        scans.add_css_class("linked");
        scans.append(&scan);
        scans.append(&scan_level);
        toolbar.append(&updates);
        toolbar.append(&scans);
        toolbar.append(&find_fixes);
        toolbar.append(&fix);
        let online = gtk4::MenuButton::new();
        online.set_label("Online");
        online.set_tooltip_text(Some("DatVault and MIA"));
        let popover = gtk4::Popover::new();
        let items = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        type MenuItem = (&'static str, fn() -> Msg);
        let menu_items: [MenuItem; 4] = [
            ("Check DatVault key", || Msg::CheckKey),
            ("Update DATs from DatVault", || Msg::DatVaultUpdate),
            ("Update MIA", || Msg::UpdateMia(false)),
            ("Update MIA (apply again even if unchanged)", || {
                Msg::UpdateMia(true)
            }),
        ];
        for (label, msg) in menu_items {
            let b = gtk4::Button::with_label(label);
            b.add_css_class("flat");
            let (s, p) = (sender.input_sender().clone(), popover.clone());
            b.connect_clicked(move |_| {
                p.popdown();
                let _ = s.send(msg());
            });
            items.append(&b);
        }
        popover.set_child(Some(&items));
        online.set_popover(Some(&popover));
        toolbar.append(&online);
        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        toolbar.append(&spacer);
        toolbar.append(&key_btn);
        toolbar.append(&settings_btn);

        // The banner: why nothing is loaded.
        let banner_text = gtk4::Label::new(None);
        banner_text.set_wrap(true);
        banner_text.set_xalign(0.0);
        banner_text.set_selectable(true);
        banner_text.add_css_class("rv-banner");
        banner_text.set_hexpand(true);
        let banner = gtk4::Revealer::new();
        banner.set_child(Some(&banner_text));

        // The tree and the lists.
        let tree = TreeView::new({
            let s = sender.input_sender().clone();
            move |e| {
                let _ = s.send(Msg::Tree(Opaque(e)));
            }
        });
        let games = GameGrid::new({
            let s = sender.input_sender().clone();
            move |e| {
                let _ = s.send(Msg::Game(Opaque(e)));
            }
        });
        let roms = RomGrid::new();

        let info_title = gtk4::Label::new(None);
        info_title.set_xalign(0.0);
        info_title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        let info_fields = gtk4::Label::new(None);
        info_fields.set_xalign(0.0);
        info_fields.set_selectable(true);
        info_fields.set_wrap(true);
        info_fields.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        info_fields.set_max_width_chars(120);
        info_fields.set_width_chars(20);
        info_fields.add_css_class("dim-label");
        let info_counts = gtk4::Label::new(None);
        info_counts.set_xalign(0.0);

        let filter_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        filter_box.set_margin_start(6);
        let filter = Rc::new(RefCell::new(prefs.filter.clone()));
        let search = gtk4::SearchEntry::new();
        search.set_placeholder_text(Some("Filter games"));
        let notify = {
            let s = sender.input_sender().clone();
            let filter = filter.clone();
            move || {
                let _ = s.send(Msg::FilterChanged(Opaque(filter.borrow().clone())));
            }
        };
        let check = |text: &str, get: fn(&GameFilter) -> bool, set: fn(&mut GameFilter, bool)| {
            let c = gtk4::CheckButton::with_label(text);
            c.set_active(get(&prefs.filter));
            let (filter, notify) = (filter.clone(), notify.clone());
            c.connect_toggled(move |c| {
                set(&mut filter.borrow_mut(), c.is_active());
                notify();
            });
            c
        };
        for c in [
            check("Complete", |f| f.complete, |f, v| f.complete = v),
            check("Partial", |f| f.partial, |f, v| f.partial = v),
            check("Empty", |f| f.empty, |f, v| f.empty = v),
            check("Fixes", |f| f.fixes, |f, v| f.fixes = v),
            check("MIA", |f| f.mia, |f, v| f.mia = v),
            check("Merged", |f| f.merged, |f, v| f.merged = v),
        ] {
            filter_box.append(&c);
        }
        {
            let (filter, notify) = (filter.clone(), notify.clone());
            search.connect_search_changed(move |e| {
                filter.borrow_mut().text = e.text().to_string();
                notify();
            });
        }
        filter_box.append(&search);

        let info = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        info.set_margin_start(6);
        info.set_margin_top(4);
        info.append(&info_title);
        info.append(&info_fields);
        info.append(&info_counts);

        let upper = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        upper.append(&info);
        upper.append(&filter_box);
        upper.append(games.widget());
        let right = gtk4::Paned::new(gtk4::Orientation::Vertical);
        right.set_start_child(Some(&upper));
        right.set_end_child(Some(roms.widget()));
        right.set_position(prefs.games_height);
        right.set_resize_start_child(true);
        right.set_resize_end_child(true);
        right.set_shrink_start_child(false);
        right.set_shrink_end_child(false);
        let main = gtk4::Paned::new(gtk4::Orientation::Horizontal);
        main.set_start_child(Some(tree.widget()));
        main.set_end_child(Some(&right));
        main.set_position(prefs.tree_width);
        main.set_resize_start_child(false);
        main.set_vexpand(true);
        main.set_shrink_start_child(false);

        // The progress panel and the status line.
        let progress = build_progress(&sender);
        let status = gtk4::Label::new(None);
        status.set_xalign(0.0);
        status.set_margin_start(6);
        status.set_margin_bottom(4);
        status.add_css_class("dim-label");

        let page = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        page.append(&toolbar);
        page.append(&banner);
        page.append(&main);
        page.append(&progress.revealer);
        page.append(&status);
        root.set_child(Some(&page));
        root.set_titlebar(Some(&gtk4::HeaderBar::new()));

        // Wire the buttons.
        let input = |msg: fn() -> Msg| {
            let s = sender.input_sender().clone();
            move |_: &gtk4::Button| {
                let _ = s.send(msg());
            }
        };
        update.connect_clicked(input(|| Msg::RunTask(Task::UpdateDats { all: false })));
        update_all.connect_clicked(input(|| Msg::RunTask(Task::UpdateDats { all: true })));
        scan.connect_clicked(input(|| Msg::ScanSelected));
        settings_btn.connect_clicked(input(|| Msg::OpenSettings(0)));
        key_btn.connect_clicked(input(|| Msg::OpenKey));
        scan_level.connect_selected_notify({
            let s = sender.input_sender().clone();
            move |d| {
                let level = match d.selected() {
                    0 => ScanLevel::Level1,
                    2 => ScanLevel::Level3,
                    _ => ScanLevel::Level2,
                };
                let _ = s.send(Msg::ScanLevelChanged(level));
            }
        });
        root.connect_close_request({
            let s = sender.input_sender().clone();
            move |_| {
                let _ = s.send(Msg::CloseRequested);
                glib::Propagation::Stop
            }
        });
        {
            let started = args.started;
            root.connect_realize(move |w| {
                once_after_paint(w, move || {
                    eprintln!(
                        "rv-gtk: first frame of the empty window at {} ms after start",
                        started.elapsed().as_millis()
                    );
                });
            });
        }

        // The working directory is the directory of the config file, as ROMVault's is where it
        // is started: every relative path in the settings is taken from it.
        let config_path = absolute(&args.config);
        if let Some(dir) = config_path.parent() {
            let _ = std::env::set_current_dir(dir);
        }

        let app = App {
            w: Widgets {
                window: root.clone(),
                tree,
                games,
                roms,
                info_title,
                info_fields,
                info_counts,
                banner,
                banner_text,
                status,
                progress,
                update,
                update_all,
                scan,
                scan_level,
                find_fixes,
                fix,
                online,
                settings: settings_btn,
            },
            prefs,
            config_path,
            settings: Settings::default(),
            db: None,
            tree_rows: Vec::new(),
            selected: None,
            task: None,
            closing: false,
            dirty: false,
            first_load: true,
            args,
        };
        app.update_buttons();
        sender.input(Msg::Reload);
        root.present();
        ComponentParts {
            model: app,
            widgets: (),
        }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>) {
        match msg {
            Msg::Reload => self.reload(&sender),
            Msg::Loaded(Opaque(result)) => {
                self.task = None;
                self.w.progress.pulse.set(self.w.progress.pulse.get() + 1);
                self.w.progress.revealer.set_reveal_child(false);
                match result {
                    Ok(f) => {
                        self.db = Some(f.db);
                        self.show_tree(f.tree);
                        self.report_load_time(f.took);
                        if let Some(n) = &f.notice {
                            self.show_banner(Some(n));
                        }
                        self.follow_args_after_load(&sender);
                    }
                    Err(e) => {
                        self.db = None;
                        self.show_banner(Some(&e));
                        self.status("The cache could not be loaded");
                        if self.args.smoke {
                            eprintln!("smoke: load failed: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                self.update_buttons();
                if self.closing {
                    self.finish_close();
                }
            }
            Msg::ScanSelected => {
                let level = self.prefs.scan_level;
                self.run_task(
                    Task::Scan {
                        level,
                        target: ScanTarget::Selected,
                    },
                    &sender,
                );
            }
            Msg::RunTask(task) => self.run_task(task, &sender),
            Msg::Progress(Opaque(e)) => self.progress(e),
            Msg::TaskDone(Opaque(f)) => {
                self.task = None;
                self.db = Some(f.db);
                self.w.progress.revealer.set_reveal_child(false);
                match f.result {
                    Ok(Some(outcome)) => {
                        let text = format!("{} ({:.1} s)", outcome.summary(), f.took.as_secs_f64());
                        self.status(&text);
                        eprintln!("rv-gtk: {text}");
                        if self.w.progress.error_count.get() > 0 {
                            self.w.progress.revealer.set_reveal_child(true);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        self.show_banner(Some(&e));
                        self.status("The operation stopped");
                    }
                }
                self.show_tree(f.tree);
                self.update_buttons();
                if self.closing {
                    self.finish_close();
                }
            }
            Msg::Cancel => {
                if let Some(t) = &self.task {
                    t.cancel.cancel();
                    self.w.progress.cancel.set_sensitive(false);
                }
            }
            Msg::Tree(Opaque(e)) => match e {
                TreeEvent::Selected(path) => {
                    self.selected = Some(path);
                    self.refresh_games(None);
                }
                TreeEvent::Tick {
                    path,
                    value,
                    just_this,
                } => {
                    if self.busy() {
                        return;
                    }
                    if let Some(db) = self.db.as_mut() {
                        if model::set_checked(&mut db.root, &path, value, just_this) {
                            self.dirty = true;
                            if let Some(node) = self.find_row(&path) {
                                self.w.tree.apply_checked(&node, value, just_this);
                            }
                        }
                    }
                }
                TreeEvent::ScanDir { path, level } => {
                    self.run_task(
                        Task::Scan {
                            level,
                            target: ScanTarget::Dir(path.join("/")),
                        },
                        &sender,
                    );
                }
            },
            Msg::Game(Opaque(GameEvent::Selected(_))) => self.refresh_roms(),
            Msg::FilterChanged(Opaque(f)) => {
                self.prefs.filter = f;
                let keep = self.w.games.selected_index();
                self.refresh_games(keep);
            }
            Msg::ScanLevelChanged(level) => self.prefs.scan_level = level,
            Msg::OpenSettings(page) => {
                if self.busy() {
                    return;
                }
                let s = sender.input_sender().clone();
                settings::show(
                    &self.w.window,
                    &self.settings,
                    &self.prefs,
                    &self.args.online,
                    page,
                    move |settings, prefs| {
                        let _ = s.send(Msg::SettingsApplied(Opaque((settings, prefs))));
                    },
                );
            }
            Msg::OpenKey => key::show(&self.w.window),
            Msg::CheckKey => match self.online_config(false) {
                Ok(cfg) => {
                    self.status("Checking the DatVault key...");
                    let tx = sender.input_sender().clone();
                    worker(move || {
                        let _ = tx.send(Msg::KeyChecked(engine::check_key(&cfg)));
                    });
                }
                Err(e) => self.show_banner(Some(&e)),
            },
            Msg::KeyChecked(text) => self.status(&text),
            Msg::DatVaultUpdate => match self.online_config(true) {
                Ok(online) => self.run_task(Task::DatVaultUpdate { online }, &sender),
                Err(e) => self.show_banner(Some(&e)),
            },
            Msg::UpdateMia(force) => match self.online_config(true) {
                Ok(online) => self.run_task(Task::UpdateMia { force, online }, &sender),
                Err(e) => self.show_banner(Some(&e)),
            },
            Msg::SettingsApplied(Opaque((settings, prefs))) => {
                match engine::check_paths(&settings, prefs.allow_protected_paths)
                    .and_then(|_| engine::write_settings(&self.config_path, &settings))
                {
                    Ok(()) => {
                        self.prefs = prefs;
                        self.save_prefs();
                        self.reload(&sender);
                    }
                    Err(e) => self.show_banner(Some(&e.to_string())),
                }
            }
            Msg::CloseRequested => {
                if let Some(t) = &self.task {
                    // Stop the operation; the window closes when it has stopped.
                    self.closing = true;
                    t.cancel.cancel();
                    self.status("Stopping before closing...");
                } else {
                    self.finish_close();
                }
            }
        }
    }
}

/// The path made absolute without needing the file to exist.
fn absolute(path: &std::path::Path) -> PathBuf {
    if let Ok(p) = path.canonicalize() {
        return p;
    }
    let name = path.file_name().unwrap_or_default();
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    dir.canonicalize()
        .map_or_else(|_| path.to_path_buf(), |d| d.join(name))
}

fn build_progress(sender: &ComponentSender<App>) -> Progress {
    let title = gtk4::Label::new(None);
    title.set_xalign(0.0);
    let item = gtk4::Label::new(None);
    item.set_xalign(0.0);
    item.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    let detail = gtk4::Label::new(None);
    detail.set_xalign(0.0);
    detail.add_css_class("dim-label");
    let overall = gtk4::ProgressBar::new();
    overall.set_show_text(true);
    let sub = gtk4::ProgressBar::new();
    sub.set_show_text(true);
    let cancel = gtk4::Button::with_label("Cancel");
    cancel.set_valign(gtk4::Align::Start);
    cancel.connect_clicked({
        let s = sender.input_sender().clone();
        move |_| {
            let _ = s.send(Msg::Cancel);
        }
    });
    let text = gtk4::TextView::new();
    text.set_editable(false);
    text.set_monospace(true);
    let errors = text.buffer();
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_child(Some(&text));
    scroll.set_min_content_height(90);
    let error_expander = gtk4::Expander::new(Some("0 problems"));
    error_expander.set_child(Some(&scroll));
    error_expander.set_visible(false);

    let bars = gtk4::Box::new(gtk4::Orientation::Vertical, 3);
    bars.set_hexpand(true);
    bars.append(&title);
    bars.append(&item);
    bars.append(&detail);
    bars.append(&overall);
    bars.append(&sub);
    bars.append(&error_expander);
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.set_margin_start(6);
    row.set_margin_end(6);
    row.set_margin_top(4);
    row.set_margin_bottom(4);
    row.append(&bars);
    row.append(&cancel);
    let frame = gtk4::Frame::new(None);
    frame.set_child(Some(&row));
    frame.set_margin_start(6);
    frame.set_margin_end(6);
    let revealer = gtk4::Revealer::new();
    revealer.set_child(Some(&frame));
    Progress {
        revealer,
        title,
        item,
        detail,
        overall,
        sub,
        cancel,
        errors,
        error_expander,
        error_count: std::cell::Cell::new(0),
        pulse: Rc::default(),
    }
}

/// The smallest the main window may be made.
const MIN_WINDOW: (i32, i32) = (1000, 600);

fn install_css() {
    let provider = gtk4::CssProvider::new();
    let mut css = crate::colours::css();
    css.push_str(
        ".rv-banner { background-color: #f8d7da; color: #58151c; padding: 8px; }\n\
         .rv-key-chip { border: 1px solid rgba(0,0,0,0.35); border-radius: 3px; padding: 2px 8px; }\n",
    );
    provider.load_from_string(&css);
    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Start the program: ask the desktop portal for file dialogs (so Plasma shows its own), build the
/// window and run until it closes.
pub fn run(args: Args) {
    if std::env::var_os("GTK_USE_PORTAL").is_none() {
        // First thing the program does, before any thread exists.
        std::env::set_var("GTK_USE_PORTAL", "1");
    }
    relm4::RelmApp::new("rs.rv.gtk")
        .with_args(vec![std::env::args()
            .next()
            .unwrap_or_else(|| "rv-gtk".to_owned())])
        .run::<App>(args);
}
