//! The settings dialog: the settings `rv-core` models (`RomVault3cfg.xml`: DAT root, cache file,
//! ROM root, ToSort, other directory mappings, DAT rules, ignore patterns, fix level) and the
//! interface's own (the default scan level, the path guard).
//!
//! The dialog edits a copy. OK checks it (the ignore patterns must compile, no path may sit in a
//! protected directory unless the guard is off) and hands it to the window, which writes the file
//! with [`crate::engine::write_settings`], so the parts of the file that are not modelled are kept.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::glib::{self, clone};
use gtk4::prelude::*;
use rv_core::guard::PROTECTED_PREFIXES;
use rv_core::{
    DatRule, DirMapping, FileType, FilterType, FixLevel, HeaderType, MergeType, RemoveSubType,
    ScanLevel, Settings, ZipStructure,
};

use super::pickers::{pick_existing_file, pick_folder, pick_new_file};
use crate::engine;
use crate::prefs::Prefs;
use rv_online::config::OnlineConfig;

type Shared = Rc<RefCell<Settings>>;

/// A rule flag: its caption, how to read it and how to set it.
type RuleFlag = (&'static str, fn(&DatRule) -> bool, fn(&mut DatRule, bool));

/// The directory a tree key is mapped to, empty when it is not mapped.
pub fn mapping_path(settings: &Settings, key: &str) -> String {
    settings
        .dir_mappings
        .iter()
        .find(|m| m.dir_key == key)
        .map(|m| m.dir_path.clone())
        .unwrap_or_default()
}

/// Map `key` to `path`; an empty path takes the mapping away.
pub fn set_mapping(settings: &mut Settings, key: &str, path: &str) {
    let path = path.trim();
    match settings.dir_mappings.iter().position(|m| m.dir_key == key) {
        Some(i) if path.is_empty() => {
            settings.dir_mappings.remove(i);
        }
        Some(i) => settings.dir_mappings[i].dir_path = path.to_owned(),
        None if !path.is_empty() => {
            settings.dir_mappings.push(DirMapping {
                dir_key: key.to_owned(),
                dir_path: path.to_owned(),
            });
            settings
                .dir_mappings
                .sort_by(|a, b| a.dir_key.cmp(&b.dir_key));
        }
        None => {}
    }
}

/// Lines of a text box as a list: trimmed, without blank lines.
pub fn lines_of(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A drop-down over `(label, value)` pairs that tells `on_change` the value picked.
fn dropdown<T: Copy + PartialEq + 'static>(
    items: &'static [(&'static str, T)],
    current: T,
    on_change: impl Fn(T) + 'static,
) -> gtk4::DropDown {
    let labels: Vec<&str> = items.iter().map(|(l, _)| *l).collect();
    let d = gtk4::DropDown::from_strings(&labels);
    d.set_selected(
        items
            .iter()
            .position(|(_, v)| *v == current)
            .map_or(gtk4::INVALID_LIST_POSITION, |p| p as u32),
    );
    d.connect_selected_notify(move |d| {
        if let Some((_, v)) = items.get(d.selected() as usize) {
            on_change(*v);
        }
    });
    d
}

fn labelled(grid: &gtk4::Grid, row: i32, text: &str, widget: &impl IsA<gtk4::Widget>) {
    let l = gtk4::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_valign(gtk4::Align::Start);
    grid.attach(&l, 0, row, 1, 1);
    widget.set_hexpand(true);
    grid.attach(widget, 1, row, 1, 1);
}

fn grid() -> gtk4::Grid {
    let g = gtk4::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(12);
    g.set_margin_start(12);
    g.set_margin_end(12);
    g.set_margin_top(12);
    g.set_margin_bottom(12);
    g
}

/// An entry with a Browse button.
fn path_row(
    window: &gtk4::Window,
    title: &'static str,
    value: &str,
    folder: bool,
    on_change: impl Fn(String) + Clone + 'static,
) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let entry = gtk4::Entry::new();
    entry.set_text(value);
    entry.set_hexpand(true);
    entry.connect_changed({
        let on_change = on_change.clone();
        move |e| on_change(e.text().to_string())
    });
    let browse = gtk4::Button::with_label("Browse...");
    browse.connect_clicked(clone!(
        #[weak]
        window,
        #[weak]
        entry,
        move |_| {
            let current = entry.text().to_string();
            let set = move |p: std::path::PathBuf| entry.set_text(&p.to_string_lossy());
            if folder {
                pick_folder(&window, title, &current, set);
            } else {
                pick_new_file(&window, title, &current, set);
            }
        }
    ));
    row.append(&entry);
    row.append(&browse);
    row
}

fn text_box(initial: &[String], on_change: impl Fn(Vec<String>) + 'static) -> gtk4::ScrolledWindow {
    let view = gtk4::TextView::new();
    view.set_monospace(true);
    view.buffer().set_text(&initial.join("\n"));
    view.buffer().connect_changed(move |b| {
        on_change(lines_of(&b.text(&b.start_iter(), &b.end_iter(), false)));
    });
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_child(Some(&view));
    scroll.set_min_content_height(110);
    scroll.set_hexpand(true);
    scroll.set_has_frame(true);
    scroll
}

const FIX_LEVELS: &[(&str, FixLevel)] = &[
    ("Level 1: copy when the CRC matches", FixLevel::Level1),
    ("Level 2: copy when the SHA1 was scanned", FixLevel::Level2),
    ("Level 3: unpack, hash and pack again", FixLevel::Level3),
];
const SCAN_LEVELS: &[(&str, ScanLevel)] = &[
    ("Quick (headers only)", ScanLevel::Level1),
    ("Normal", ScanLevel::Level2),
    ("Full (re-scan)", ScanLevel::Level3),
];
const COMPRESSION: &[(&str, FileType)] = &[
    ("Zip", FileType::ZIP),
    ("7z", FileType::SEVEN_ZIP),
    ("Files only", FileType::FILE_ONLY),
];
const STRUCTURE: &[(&str, ZipStructure)] = &[
    ("None", ZipStructure::NONE),
    ("Zip: TorrentZip", ZipStructure::ZIP_TRRNT),
    ("Zip: TDC", ZipStructure::ZIP_TDC),
    ("Zip: DTD", ZipStructure::ZIP_DTD),
    ("Zip: Zstandard", ZipStructure::ZIP_ZSTD),
    ("Zip: DTZ", ZipStructure::ZIP_DTZ),
    ("7z: Trrnt", ZipStructure::SEVEN_ZIP_TRRNT),
    ("7z: solid LZMA", ZipStructure::SEVEN_ZIP_SLZMA),
    ("7z: non-solid LZMA", ZipStructure::SEVEN_ZIP_NLZMA),
    ("7z: solid Zstandard", ZipStructure::SEVEN_ZIP_SZSTD),
    ("7z: non-solid Zstandard", ZipStructure::SEVEN_ZIP_NZSTD),
];
const MERGE: &[(&str, MergeType)] = &[
    ("Leave as the DAT has it", MergeType::None),
    ("Split", MergeType::Split),
    ("Merge", MergeType::Merge),
    ("Non-merged", MergeType::NonMerged),
];
const FILTER: &[(&str, FilterType)] = &[
    ("Keep all", FilterType::KeepAll),
    ("ROMs only", FilterType::RomsOnly),
    ("CHDs only", FilterType::ChdsOnly),
];
const HEADER: &[(&str, HeaderType)] = &[
    ("Optional", HeaderType::Optional),
    ("Headered", HeaderType::Headered),
    ("Headerless", HeaderType::Headerless),
];
const SUB_DIRS: &[(&str, RemoveSubType)] = &[
    ("Keep all sub directories", RemoveSubType::KeepAllSubDirs),
    (
        "Remove all sub directories",
        RemoveSubType::RemoveAllSubDirs,
    ),
    (
        "Remove all if no name conflicts",
        RemoveSubType::RemoveAllIfNoConflicts,
    ),
    (
        "Remove if they hold single files",
        RemoveSubType::RemoveSubIfSingleFiles,
    ),
    (
        "Remove if the name matches",
        RemoveSubType::RemoveSubIfNameMatches,
    ),
];

// ---- the DAT rule form -----------------------------------------------------------------------

fn rule_check(
    s: &Shared,
    idx: usize,
    label: &str,
    get: fn(&DatRule) -> bool,
    set: fn(&mut DatRule, bool),
) -> gtk4::CheckButton {
    let c = gtk4::CheckButton::with_label(label);
    c.set_active(get(&s.borrow().dat_rules[idx]));
    let s = s.clone();
    c.connect_toggled(move |c| {
        if let Some(rule) = s.borrow_mut().dat_rules.get_mut(idx) {
            set(rule, c.is_active());
        }
    });
    c
}

fn rule_form(s: &Shared, idx: usize) -> gtk4::Grid {
    let g = grid();
    let rule = s.borrow().dat_rules[idx].clone();
    let mut row = 0;
    let mut add = |text: &str, w: &dyn Fn() -> gtk4::Widget| {
        labelled(&g, row, text, &w());
        row += 1;
    };
    add("Archive type", &|| {
        let s = s.clone();
        dropdown(COMPRESSION, rule.compression, move |v| {
            if let Some(r) = s.borrow_mut().dat_rules.get_mut(idx) {
                r.compression = v;
            }
        })
        .upcast()
    });
    add("Structure", &|| {
        let s = s.clone();
        dropdown(STRUCTURE, rule.compression_sub, move |v| {
            if let Some(r) = s.borrow_mut().dat_rules.get_mut(idx) {
                r.compression_sub = v;
            }
        })
        .upcast()
    });
    add("Merge", &|| {
        let s = s.clone();
        dropdown(MERGE, rule.merge, move |v| {
            if let Some(r) = s.borrow_mut().dat_rules.get_mut(idx) {
                r.merge = v;
            }
        })
        .upcast()
    });
    add("Filter", &|| {
        let s = s.clone();
        dropdown(FILTER, rule.filter, move |v| {
            if let Some(r) = s.borrow_mut().dat_rules.get_mut(idx) {
                r.filter = v;
            }
        })
        .upcast()
    });
    add("Headers", &|| {
        let s = s.clone();
        dropdown(HEADER, rule.header_type, move |v| {
            if let Some(r) = s.borrow_mut().dat_rules.get_mut(idx) {
                r.header_type = v;
            }
        })
        .upcast()
    });
    add("Sub directories", &|| {
        let s = s.clone();
        dropdown(SUB_DIRS, rule.sub_dir_type, move |v| {
            if let Some(r) = s.borrow_mut().dat_rules.get_mut(idx) {
                r.sub_dir_type = v;
            }
        })
        .upcast()
    });
    let checks = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    let flags: [RuleFlag; 9] = [
        (
            "Use this archive type instead of the DAT's",
            |r| r.compression_override_dat,
            |r, v| r.compression_override_dat = v,
        ),
        (
            "Convert files while fixing",
            |r| r.convert_while_fixing,
            |r, v| r.convert_while_fixing = v,
        ),
        (
            "Use this merge instead of the DAT's",
            |r| r.merge_override_dat,
            |r, v| r.merge_override_dat = v,
        ),
        (
            "One archive per DAT",
            |r| r.single_archive,
            |r, v| r.single_archive = v,
        ),
        (
            "Multiple DATs in one directory",
            |r| r.multi_dat_dir_override,
            |r, v| r.multi_dat_dir_override = v,
        ),
        (
            "Name directories after the DAT description",
            |r| r.use_description_as_dir_name,
            |r, v| r.use_description_as_dir_name = v,
        ),
        (
            "Name directories after the DAT id",
            |r| r.use_id_for_name,
            |r, v| r.use_id_for_name = v,
        ),
        (
            "Complete sets only",
            |r| r.complete_only,
            |r, v| r.complete_only = v,
        ),
        (
            "Add category sub directories",
            |r| r.add_category_sub_dirs,
            |r, v| r.add_category_sub_dirs = v,
        ),
    ];
    for (label, get, set) in flags {
        checks.append(&rule_check(s, idx, label, get, set));
    }
    g.attach(&checks, 0, row, 2, 1);
    row += 1;
    let ignore = text_box(&rule.ignore_files, {
        let s = s.clone();
        move |v| {
            if let Some(r) = s.borrow_mut().dat_rules.get_mut(idx) {
                r.ignore_files = v;
            }
        }
    });
    labelled(&g, row, "Files to keep (one pattern per line)", &ignore);
    row += 1;
    let order = text_box(&rule.category_order, {
        let s = s.clone();
        move |v| {
            if let Some(r) = s.borrow_mut().dat_rules.get_mut(idx) {
                r.category_order = v;
            }
        }
    });
    labelled(&g, row, "Category order (one per line)", &order);
    g
}

fn rules_page(s: &Shared) -> gtk4::Paned {
    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::Single);
    let form = gtk4::ScrolledWindow::new();
    form.set_hexpand(true);
    form.set_vexpand(true);

    let fill = {
        let (s, list) = (s.clone(), list.clone());
        move |select: Option<usize>| {
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            for rule in &s.borrow().dat_rules {
                let l = gtk4::Label::new(Some(&rule.dir_key));
                l.set_xalign(0.0);
                l.set_margin_start(6);
                l.set_margin_end(6);
                l.set_margin_top(3);
                l.set_margin_bottom(3);
                list.append(&l);
            }
            if let Some(row) = select.and_then(|i| list.row_at_index(i as i32)) {
                list.select_row(Some(&row));
            }
        }
    };
    list.connect_row_selected(clone!(
        #[strong]
        s,
        #[weak]
        form,
        move |_, row| {
            match row
                .map(|r| r.index() as usize)
                .filter(|i| *i < s.borrow().dat_rules.len())
            {
                Some(i) => form.set_child(Some(&rule_form(&s, i))),
                None => form.set_child(None::<&gtk4::Widget>),
            }
        }
    ));
    // Selecting the first rule shows its form: the handler above builds it.
    fill(Some(0));

    let new_key = gtk4::Entry::new();
    new_key.set_placeholder_text(Some("RomVault/Console/NES"));
    let add = gtk4::Button::with_label("Add rule");
    let remove = gtk4::Button::with_label("Remove");
    add.connect_clicked(clone!(
        #[strong]
        s,
        #[weak]
        new_key,
        #[strong]
        fill,
        move |_| {
            let key = new_key.text().trim().replace('\\', "/");
            let key = key.trim_end_matches('/').to_owned();
            if !key.starts_with("RomVault") || s.borrow().dat_rules.iter().any(|r| r.dir_key == key)
            {
                new_key.add_css_class("error");
                return;
            }
            new_key.remove_css_class("error");
            let at = {
                let mut st = s.borrow_mut();
                let mut rule = DatRule::default();
                rule.dir_key = key.clone();
                st.dat_rules.push(rule);
                st.dat_rules.sort_by(|a, b| a.dir_key.cmp(&b.dir_key));
                st.dat_rules.iter().position(|r| r.dir_key == key)
            };
            new_key.set_text("");
            fill(at);
        }
    ));
    remove.connect_clicked(clone!(
        #[strong]
        s,
        #[weak]
        list,
        #[strong]
        fill,
        move |_| {
            let Some(i) = list.selected_row().map(|r| r.index() as usize) else {
                return;
            };
            let mut st = s.borrow_mut();
            // The rule for the whole tree is the fallback for every DAT and stays.
            if st.dat_rules.get(i).is_some_and(|r| r.dir_key != "RomVault") {
                st.dat_rules.remove(i);
                drop(st);
                fill(Some(i.saturating_sub(1)));
            }
        }
    ));
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_child(Some(&list));
    scroll.set_vexpand(true);
    scroll.set_min_content_width(260);
    let left = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    left.set_margin_start(6);
    left.set_margin_top(6);
    left.set_margin_bottom(6);
    left.append(&scroll);
    left.append(&new_key);
    let buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    buttons.append(&add);
    buttons.append(&remove);
    left.append(&buttons);
    let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    paned.set_start_child(Some(&left));
    paned.set_end_child(Some(&form));
    paned.set_position(300);
    paned
}

// ---- the directories page --------------------------------------------------------------------

/// The mappings other than the two on the General page.
fn directories_page(window: &gtk4::Window, s: &Shared) -> gtk4::Box {
    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    page.set_margin_start(12);
    page.set_margin_end(12);
    page.set_margin_top(12);
    let note = gtk4::Label::new(Some(
        "Where other tree directories live on disk. The directory with the longest matching key is used.",
    ));
    note.set_xalign(0.0);
    note.set_wrap(true);
    page.append(&note);
    let rows = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_child(Some(&rows));
    scroll.set_vexpand(true);
    page.append(&scroll);

    type Rebuild = Rc<RefCell<Option<Rc<dyn Fn()>>>>;
    let rebuild: Rebuild = Rc::default();
    let build = {
        let (s, rows, window, rebuild) = (s.clone(), rows.clone(), window.clone(), rebuild.clone());
        move || {
            while let Some(c) = rows.first_child() {
                rows.remove(&c);
            }
            let keys: Vec<String> = s
                .borrow()
                .dir_mappings
                .iter()
                .map(|m| m.dir_key.clone())
                .filter(|k| k != "RomVault" && k != "ToSort")
                .collect();
            for key in keys {
                let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
                let k = gtk4::Label::new(Some(&key));
                k.set_xalign(0.0);
                k.set_width_chars(34);
                k.set_ellipsize(gtk4::pango::EllipsizeMode::Start);
                let path = path_row(
                    &window,
                    "Choose the directory",
                    &mapping_path(&s.borrow(), &key),
                    true,
                    {
                        let (s, key) = (s.clone(), key.clone());
                        move |p| set_mapping_keep(&mut s.borrow_mut(), &key, &p)
                    },
                );
                let del = gtk4::Button::with_label("Remove");
                del.connect_clicked({
                    let (s, key, rebuild) = (s.clone(), key.clone(), rebuild.clone());
                    move |_| {
                        set_mapping(&mut s.borrow_mut(), &key, "");
                        if let Some(f) = rebuild.borrow().clone() {
                            f();
                        }
                    }
                });
                row.append(&k);
                path.set_hexpand(true);
                row.append(&path);
                row.append(&del);
                rows.append(&row);
            }
        }
    };
    let build: Rc<dyn Fn()> = Rc::new(build);
    *rebuild.borrow_mut() = Some(build.clone());
    build();

    let add_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let key = gtk4::Entry::new();
    key.set_placeholder_text(Some("RomVault/Console/NES"));
    key.set_width_chars(34);
    let path = gtk4::Entry::new();
    path.set_placeholder_text(Some("/path/to/directory"));
    path.set_hexpand(true);
    let add = gtk4::Button::with_label("Add");
    add.connect_clicked(clone!(
        #[strong]
        s,
        #[weak]
        key,
        #[weak]
        path,
        move |_| {
            let k = key.text().trim().replace('\\', "/");
            let k = k.trim_end_matches('/').to_owned();
            let p = path.text().trim().to_owned();
            if k.is_empty()
                || p.is_empty()
                || !(k == "RomVault" || k.starts_with("RomVault/") || k.starts_with("ToSort"))
            {
                key.add_css_class("error");
                return;
            }
            key.remove_css_class("error");
            set_mapping(&mut s.borrow_mut(), &k, &p);
            key.set_text("");
            path.set_text("");
            build();
        }
    ));
    add_row.append(&key);
    add_row.append(&path);
    add_row.append(&add);
    add_row.set_margin_bottom(12);
    page.append(&add_row);
    page
}

/// Like [`set_mapping`] but an emptied entry keeps the mapping with an empty path while the user
/// is typing (the row stays; the dialog drops empty ones on OK).
fn set_mapping_keep(settings: &mut Settings, key: &str, path: &str) {
    if let Some(m) = settings.dir_mappings.iter_mut().find(|m| m.dir_key == key) {
        m.dir_path = path.to_owned();
    }
}

// ---- the dialog ------------------------------------------------------------------------------

fn general_page(window: &gtk4::Window, s: &Shared, p: &Rc<RefCell<Prefs>>) -> gtk4::Grid {
    let g = grid();
    let st = s.borrow().clone();
    let mut row = 0;
    let mut put = |text: &str, w: gtk4::Widget| {
        labelled(&g, row, text, &w);
        row += 1;
    };
    put(
        "DAT root",
        path_row(window, "Choose the DAT root", &st.dat_root, true, {
            let s = s.clone();
            move |v| s.borrow_mut().dat_root = v
        })
        .upcast(),
    );
    put(
        "ROM root (RomVault)",
        path_row(
            window,
            "Choose the ROM root",
            &mapping_path(&st, "RomVault"),
            true,
            {
                let s = s.clone();
                move |v| set_mapping(&mut s.borrow_mut(), "RomVault", &v)
            },
        )
        .upcast(),
    );
    put(
        "ToSort",
        path_row(
            window,
            "Choose the ToSort directory",
            &mapping_path(&st, "ToSort"),
            true,
            {
                let s = s.clone();
                move |v| set_mapping(&mut s.borrow_mut(), "ToSort", &v)
            },
        )
        .upcast(),
    );
    put(
        "Cache file",
        path_row(window, "Choose the cache file", &st.cache_file, false, {
            let s = s.clone();
            move |v| s.borrow_mut().cache_file = v
        })
        .upcast(),
    );
    put(
        "Fixing level",
        dropdown(FIX_LEVELS, st.fix_level, {
            let s = s.clone();
            move |v| s.borrow_mut().fix_level = v
        })
        .upcast(),
    );
    put(
        "Scan level for the Scan button",
        dropdown(SCAN_LEVELS, p.borrow().scan_level, {
            let p = p.clone();
            move |v| p.borrow_mut().scan_level = v
        })
        .upcast(),
    );

    let timer = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let on = gtk4::CheckButton::with_label("Save the cache on a timer every");
    on.set_active(st.cache_save_timer_enabled);
    on.connect_toggled({
        let s = s.clone();
        move |c| s.borrow_mut().cache_save_timer_enabled = c.is_active()
    });
    let minutes = gtk4::SpinButton::with_range(1.0, 600.0, 1.0);
    minutes.set_value(f64::from(st.cache_save_time_period));
    minutes.connect_value_changed({
        let s = s.clone();
        move |m| s.borrow_mut().cache_save_time_period = m.value() as i32
    });
    timer.append(&on);
    timer.append(&minutes);
    timer.append(&gtk4::Label::new(Some("minutes")));
    put("", timer.upcast());

    let chd = gtk4::CheckButton::with_label("Check the version of CHD files");
    chd.set_active(st.check_chd_version);
    chd.connect_toggled({
        let s = s.clone();
        move |c| s.borrow_mut().check_chd_version = c.is_active()
    });
    put("", chd.upcast());
    let mia = gtk4::CheckButton::with_label("Show the Newly Missing status");
    mia.set_active(st.show_new_mia);
    mia.connect_toggled({
        let s = s.clone();
        move |c| s.borrow_mut().show_new_mia = c.is_active()
    });
    put("", mia.upcast());

    let guard = gtk4::CheckButton::with_label(&format!(
        "Allow paths under {}",
        PROTECTED_PREFIXES.join(", ")
    ));
    guard.set_active(p.borrow().allow_protected_paths);
    guard.connect_toggled({
        let p = p.clone();
        move |c| p.borrow_mut().allow_protected_paths = c.is_active()
    });
    guard.set_tooltip_text(Some(
        "These directories are refused by default so that a copy of someone's settings cannot touch their real collection.",
    ));
    put("Safety", guard.upcast());
    g
}

type SharedOnline = Rc<RefCell<OnlineConfig>>;

/// The Online page: the DatVault key and the MIA settings, kept in `online.conf`, not in
/// `RomVault3cfg.xml`. The key is hidden while typed and never leaves this form except through
/// the settings file (owner only) and the key check.
fn online_page(window: &gtk4::Window, o: &SharedOnline, path: &std::path::Path) -> gtk4::Grid {
    let g = grid();
    let cfg = o.borrow().clone();
    let mut row = 0;
    let mut put = |text: &str, w: gtk4::Widget| {
        labelled(&g, row, text, &w);
        row += 1;
    };
    let key = gtk4::PasswordEntry::new();
    key.set_show_peek_icon(true);
    key.set_text(&cfg.dat_vault_key);
    key.connect_changed({
        let o = o.clone();
        move |e| o.borrow_mut().dat_vault_key = e.text().to_string()
    });
    put("DatVault key", key.upcast());

    let check = gtk4::Button::with_label("Check key");
    let result = gtk4::Label::new(None);
    result.set_xalign(0.0);
    result.set_wrap(true);
    result.set_selectable(true);
    check.connect_clicked(clone!(
        #[weak]
        result,
        #[strong]
        o,
        move |b| {
            let c = o.borrow().clone();
            result.set_text("Checking...");
            b.set_sensitive(false);
            let b = b.clone();
            glib::spawn_future_local(async move {
                let text = gtk4::gio::spawn_blocking(move || engine::check_key(&c))
                    .await
                    .unwrap_or_else(|_| "The check stopped.".to_owned());
                result.set_text(&text);
                b.set_sensitive(true);
            });
        }
    ));
    let checks = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    checks.append(&check);
    checks.append(&result);
    put("", checks.upcast());

    let exe_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let exe = gtk4::Entry::new();
    exe.set_hexpand(true);
    exe.set_text(
        &cfg.rom_vault_exe
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    exe.set_placeholder_text(Some("ROMVault38.exe"));
    exe.connect_changed({
        let o = o.clone();
        move |e| {
            let t = e.text().to_string();
            o.borrow_mut().rom_vault_exe = (!t.trim().is_empty()).then(|| t.into());
        }
    });
    let browse = gtk4::Button::with_label("Browse...");
    browse.connect_clicked(clone!(
        #[weak]
        window,
        #[weak]
        exe,
        move |_| {
            let current = exe.text().to_string();
            pick_existing_file(
                &window,
                "Choose your ROMVault 3.8.0 exe",
                &current,
                move |p| exe.set_text(&p.to_string_lossy()),
            );
        }
    ));
    exe_row.append(&exe);
    exe_row.append(&browse);
    exe_row.set_tooltip_text(Some(
        "The MIA list is encrypted with a value that is read from your own ROMVault38.exe at run time. Nothing is copied from it.",
    ));
    put("ROMVault 3.8.0 exe (for MIA)", exe_row.upcast());

    let days = gtk4::SpinButton::with_range(1.0, 3650.0, 1.0);
    days.set_value(cfg.mia_days as f64);
    days.connect_value_changed({
        let o = o.clone();
        move |d| o.borrow_mut().mia_days = d.value() as i64
    });
    put("Days a missing file counts as new", days.upcast());

    let cue = gtk4::CheckButton::with_label("Delete the old cue file when a DAT is updated");
    cue.set_active(cfg.delete_old_cue_files);
    cue.connect_toggled({
        let o = o.clone();
        move |c| o.borrow_mut().delete_old_cue_files = c.is_active()
    });
    put("", cue.upcast());

    let addr = gtk4::Entry::new();
    addr.set_placeholder_text(Some("blank: the default https:// address"));
    addr.set_text(cfg.endpoint.as_deref().unwrap_or(""));
    addr.connect_changed({
        let o = o.clone();
        move |e| {
            let t = e.text().to_string();
            o.borrow_mut().endpoint = (!t.trim().is_empty()).then_some(t);
        }
    });
    put("Service address (advanced)", addr.upcast());
    let plain = gtk4::CheckButton::with_label(
        "Allow a plain http:// address (the key is sent unencrypted)",
    );
    plain.set_active(cfg.allow_plain_http);
    plain.connect_toggled({
        let o = o.clone();
        move |c| o.borrow_mut().allow_plain_http = c.is_active()
    });
    put("", plain.upcast());

    let note = gtk4::Label::new(Some(&format!(
        "Saved in {}, readable by you only. Reporting found MIA files is off and is not part of this window.",
        path.display()
    )));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.add_css_class("dim-label");
    put("", note.upcast());
    g
}

pub fn show(
    parent: &gtk4::Window,
    settings: &Settings,
    prefs: &Prefs,
    online_path: &std::path::Path,
    page: u32,
    apply: impl Fn(Settings, Prefs) + 'static,
) {
    let window = gtk4::Window::builder()
        .title("Settings")
        .transient_for(parent)
        .modal(true)
        .default_width(820)
        .default_height(720)
        .build();
    let s: Shared = Rc::new(RefCell::new(settings.clone()));
    let p = Rc::new(RefCell::new(prefs.clone()));

    let book = gtk4::Notebook::new();
    book.append_page(
        &general_page(&window, &s, &p),
        Some(&gtk4::Label::new(Some("General"))),
    );
    book.append_page(
        &directories_page(&window, &s),
        Some(&gtk4::Label::new(Some("Directories"))),
    );
    book.append_page(&rules_page(&s), Some(&gtk4::Label::new(Some("DAT rules"))));
    let ignore = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    ignore.set_margin_start(12);
    ignore.set_margin_end(12);
    ignore.set_margin_top(12);
    let help = gtk4::Label::new(Some(
        "Files that are never removed and shown as Ignore. One pattern per line.\n\
         * and ? are wildcards; start with regex: for a regular expression, or with ignore: to \
         not scan the file at all.",
    ));
    help.set_xalign(0.0);
    help.set_wrap(true);
    ignore.append(&help);
    let patterns = text_box(&settings.ignore_files, {
        let s = s.clone();
        move |v| s.borrow_mut().ignore_files = v
    });
    patterns.set_vexpand(true);
    ignore.append(&patterns);
    book.append_page(&ignore, Some(&gtk4::Label::new(Some("Ignore"))));
    let o: SharedOnline = Rc::new(RefCell::new(
        OnlineConfig::load(online_path).unwrap_or_default(),
    ));
    book.append_page(
        &online_page(&window, &o, online_path),
        Some(&gtk4::Label::new(Some("Online"))),
    );
    book.set_vexpand(true);
    book.set_current_page(Some(page));
    let online_path = online_path.to_path_buf();

    let error = gtk4::Label::new(None);
    error.set_xalign(0.0);
    error.set_wrap(true);
    error.set_selectable(true);
    error.add_css_class("error");
    let ok = gtk4::Button::with_label("OK");
    ok.add_css_class("suggested-action");
    let cancel = gtk4::Button::with_label("Cancel");
    let buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    buttons.set_halign(gtk4::Align::End);
    buttons.append(&cancel);
    buttons.append(&ok);

    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    page.set_margin_start(8);
    page.set_margin_end(8);
    page.set_margin_top(8);
    page.set_margin_bottom(8);
    page.append(&book);
    page.append(&error);
    page.append(&buttons);
    window.set_child(Some(&page));

    cancel.connect_clicked(clone!(
        #[weak]
        window,
        move |_| window.close()
    ));
    ok.connect_clicked(clone!(
        #[weak]
        window,
        #[weak]
        error,
        move |_| {
            let mut settings = s.borrow().clone();
            let prefs = p.borrow().clone();
            // A mapping that was emptied while typing is no mapping.
            settings
                .dir_mappings
                .retain(|m| !m.dir_path.trim().is_empty());
            match validate(&mut settings, &prefs)
                .and_then(|()| o.borrow().save(&online_path).map_err(|e| e.to_string()))
            {
                Ok(()) => {
                    apply(settings, prefs);
                    window.close();
                }
                Err(e) => error.set_text(&e),
            }
        }
    ));
    window.present();
}

/// What OK checks: the ignore patterns compile and no path is in a protected directory.
pub fn validate(settings: &mut Settings, prefs: &Prefs) -> Result<(), String> {
    settings.set_regex_rules().map_err(|e| e.to_string())?;
    if settings.dat_root.trim().is_empty() {
        return Err("The DAT root is empty.".to_owned());
    }
    if settings.cache_file.trim().is_empty() {
        return Err("The cache file is empty.".to_owned());
    }
    engine::check_paths(settings, prefs.allow_protected_paths).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mapping_is_set_changed_and_taken_away() {
        let mut s = Settings::default();
        assert_eq!(mapping_path(&s, "ToSort"), "");
        set_mapping(&mut s, "ToSort", "/data/ToSort");
        assert_eq!(mapping_path(&s, "ToSort"), "/data/ToSort");
        set_mapping(&mut s, "ToSort", "/other");
        assert_eq!(
            s.dir_mappings
                .iter()
                .filter(|m| m.dir_key == "ToSort")
                .count(),
            1
        );
        assert_eq!(mapping_path(&s, "ToSort"), "/other");
        set_mapping(&mut s, "ToSort", "  ");
        assert_eq!(mapping_path(&s, "ToSort"), "");
        // Setting nothing on a key that is not there adds nothing.
        let n = s.dir_mappings.len();
        set_mapping(&mut s, "RomVault/Other", "");
        assert_eq!(s.dir_mappings.len(), n);
    }

    #[test]
    fn text_boxes_become_lists_without_blank_lines() {
        assert_eq!(
            lines_of(" *.bak \n\n regex:^a$\n  \n"),
            ["*.bak", "regex:^a$"]
        );
        assert!(lines_of("").is_empty());
    }

    #[test]
    fn ok_refuses_a_pattern_that_does_not_compile_and_a_protected_path() {
        let prefs = Prefs::default();
        let mut s = Settings::default();
        assert!(validate(&mut s, &prefs).is_ok());

        let mut bad = Settings::default();
        bad.ignore_files = vec!["regex:(unclosed".to_owned()];
        assert!(validate(&mut bad, &prefs)
            .unwrap_err()
            .contains("ignore pattern"));

        let mut fenced = Settings::default();
        fenced.dat_root = "/mnt/HHD16TB/DatRoot".to_owned();
        let e = validate(&mut fenced, &prefs).unwrap_err();
        assert!(e.contains("protected directory"), "{e}");

        // No test turns the guard off: the default is on.
        assert!(!prefs.allow_protected_paths);
    }

    #[test]
    fn the_dialogs_option_lists_cover_what_rv_core_reads() {
        // Every default rule value is offered, so opening a rule never shows an empty choice.
        let r = DatRule::default();
        assert!(COMPRESSION.iter().any(|(_, v)| *v == r.compression));
        assert!(STRUCTURE.iter().any(|(_, v)| *v == r.compression_sub));
        assert!(MERGE.iter().any(|(_, v)| *v == r.merge));
        assert!(FILTER.iter().any(|(_, v)| *v == r.filter));
        assert!(HEADER.iter().any(|(_, v)| *v == r.header_type));
        assert!(SUB_DIRS.iter().any(|(_, v)| *v == r.sub_dir_type));
        assert!(FIX_LEVELS
            .iter()
            .any(|(_, v)| *v == Settings::default().fix_level));
    }
}
