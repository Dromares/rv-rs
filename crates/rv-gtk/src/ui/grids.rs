//! The game list and the ROM list: `ColumnView`s over [`VirtualList`] models, so that a
//! directory of tens of thousands of games costs a handful of widgets and nothing is built for
//! the rows that are not on screen.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::glib::{self, clone};
use gtk4::pango;
use gtk4::prelude::*;
use rv_core::RepStatus;

use super::icons::{draw_got, draw_strip, strip_width, GOT_SIZE};
use super::painted::Painted;
use super::virtual_list::{row_of, VecSource, VirtualList};
use crate::colours::css_class;
use crate::model::{sort_games, GameList, GameRow, GameSort, RomList, RomRow};

/// Put the colour class of `status` on `label` and take every other status class off it.
fn paint_label(label: &gtk4::Label, status: Option<RepStatus>) {
    for class in label.css_classes() {
        if class.starts_with("rv-st-") {
            label.remove_css_class(&class);
        }
    }
    if let Some(s) = status {
        label.add_css_class(&css_class(s));
    }
}

/// A column of plain text cells.
fn text_column<T: Clone + 'static>(
    title: &str,
    width: i32,
    expand: bool,
    text: fn(&T) -> String,
    colour: fn(&T) -> Option<RepStatus>,
) -> gtk4::ColumnViewColumn {
    let factory = gtk4::SignalListItemFactory::new();
    factory.connect_setup(|_, obj| {
        let li = obj.downcast_ref::<gtk4::ListItem>().expect("a list item");
        let label = gtk4::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(pango::EllipsizeMode::End);
        label.set_hexpand(true);
        label.set_halign(gtk4::Align::Fill);
        label.set_margin_start(4);
        li.set_child(Some(&label));
    });
    factory.connect_bind(move |_, obj| {
        let li = obj.downcast_ref::<gtk4::ListItem>().expect("a list item");
        let label = li.child().and_downcast::<gtk4::Label>().expect("a label");
        if let Some(row) = li.item().and_then(|i| row_of::<T>(&i)) {
            label.set_text(&text(&row));
            paint_label(&label, colour(&row));
        }
    });
    let column = gtk4::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_resizable(true);
    column.set_fixed_width(width);
    column.set_expand(expand);
    column
}

/// What the game list tells the window.
#[derive(Clone, Debug)]
pub enum GameEvent {
    /// The game selected, as its position among the directory's children; `None` when nothing is.
    Selected(Option<usize>),
}

pub struct GameGrid {
    scroll: gtk4::ScrolledWindow,
    view: gtk4::ColumnView,
    list: VirtualList,
    selection: gtk4::SingleSelection,
    rows: RefCell<Vec<GameRow>>,
    description: gtk4::ColumnViewColumn,
    sort: Cell<Option<(GameSort, bool)>>,
    quiet: Rc<Cell<bool>>,
}

impl GameGrid {
    pub fn new(events: impl Fn(GameEvent) + Clone + 'static) -> Rc<GameGrid> {
        let list = VirtualList::new();
        let selection = gtk4::SingleSelection::new(Some(list.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        let view = gtk4::ColumnView::new(Some(selection.clone()));
        view.set_show_column_separators(true);
        view.set_show_row_separators(false);

        // Type and the sort-by columns get a sorter that does nothing, so that the header can be
        // clicked and says which column; the sort itself is done on the rows.
        let sortable = |c: &gtk4::ColumnViewColumn| {
            c.set_sorter(Some(&gtk4::CustomSorter::new(|_, _| gtk4::Ordering::Equal)));
        };
        let kind = text_column::<GameRow>("Type", 56, false, |r| r.kind.to_owned(), |_| None);
        let name = text_column::<GameRow>(
            "Game (Directory / Zip)",
            330,
            false,
            |r| r.name.clone(),
            |_| None,
        );
        let description = text_column::<GameRow>(
            "Description",
            260,
            false,
            |r| r.description.clone(),
            |_| None,
        );
        let modified = text_column::<GameRow>(
            "Modified",
            170,
            false,
            |r| crate::model::time_text(r.modified),
            |_| None,
        );
        for c in [&kind, &name, &description, &modified] {
            sortable(c);
        }

        let strip_factory = gtk4::SignalListItemFactory::new();
        strip_factory.connect_setup(|_, obj| {
            let li = obj.downcast_ref::<gtk4::ListItem>().expect("a list item");
            let strip = Painted::new(120, 20);
            strip.set_halign(gtk4::Align::Start);
            li.set_child(Some(&strip));
        });
        strip_factory.connect_bind(|_, obj| {
            let li = obj.downcast_ref::<gtk4::ListItem>().expect("a list item");
            let strip = li.child().and_downcast::<Painted>().expect("a strip");
            if let Some(row) = li.item().and_then(|i| row_of::<GameRow>(&i)) {
                strip.set_content_width(strip_width(&row.status));
                let status = row.status;
                strip.set_painter(Box::new(move |cr, _, h| {
                    draw_strip(cr, f64::from(h), &status)
                }));
            }
        });
        let strip = gtk4::ColumnViewColumn::new(Some("ROM Status"), Some(strip_factory));
        strip.set_resizable(true);
        strip.set_expand(true);

        for c in [&kind, &name, &description, &modified, &strip] {
            view.append_column(c);
        }

        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_child(Some(&view));
        scroll.set_hexpand(true);
        scroll.set_vexpand(true);

        let grid = Rc::new(GameGrid {
            scroll,
            view: view.clone(),
            list,
            selection: selection.clone(),
            rows: RefCell::new(Vec::new()),
            description,
            sort: Cell::new(None),
            quiet: Rc::new(Cell::new(false)),
        });

        let columns = [
            (kind, GameSort::Kind),
            (name, GameSort::Name),
            (grid.description.clone(), GameSort::Description),
            (modified, GameSort::Modified),
        ];
        if let Some(sorter) = view.sorter() {
            sorter.connect_changed(clone!(
                #[weak]
                grid,
                #[weak]
                view,
                move |_, _| {
                    let Some(s) = view.sorter().and_downcast::<gtk4::ColumnViewSorter>() else {
                        return;
                    };
                    let by = s
                        .primary_sort_column()
                        .and_then(|c| columns.iter().find(|(col, _)| *col == c).map(|(_, by)| *by));
                    let desc = s.primary_sort_order() == gtk4::SortType::Descending;
                    grid.sort.set(by.map(|b| (b, desc)));
                    grid.resort();
                }
            ));
        }
        selection.connect_selected_notify(clone!(
            #[weak]
            grid,
            move |sel| {
                if grid.quiet.get() {
                    return;
                }
                let index = (sel.selected() != gtk4::INVALID_LIST_POSITION)
                    .then(|| {
                        grid.rows
                            .borrow()
                            .get(sel.selected() as usize)
                            .map(|r| r.index)
                    })
                    .flatten();
                events(GameEvent::Selected(index));
            }
        ));
        grid
    }

    pub fn widget(&self) -> &gtk4::ScrolledWindow {
        &self.scroll
    }

    /// The game selected, as its position among the directory's children.
    pub fn selected_index(&self) -> Option<usize> {
        let s = self.selection.selected();
        (s != gtk4::INVALID_LIST_POSITION)
            .then(|| self.rows.borrow().get(s as usize).map(|r| r.index))
            .flatten()
    }

    /// Show a new list; keep the selected game if it is in it. Returns whether the selection
    /// changed, which the caller has to follow with the ROM list.
    pub fn set_list(&self, list: GameList, keep: Option<usize>) {
        self.description.set_visible(list.show_description);
        let mut rows = list.rows;
        if let Some((by, desc)) = self.sort.get() {
            sort_games(&mut rows, by, desc);
        }
        self.install(rows, keep);
    }

    fn resort(&self) {
        let keep = self.selected_index();
        let mut rows = self.rows.borrow().clone();
        if let Some((by, desc)) = self.sort.get() {
            sort_games(&mut rows, by, desc);
        }
        self.install(rows, keep);
    }

    fn install(&self, rows: Vec<GameRow>, keep: Option<usize>) {
        let position = keep.and_then(|k| rows.iter().position(|r| r.index == k));
        *self.rows.borrow_mut() = rows.clone();
        self.quiet.set(true);
        self.list.set_source(Some(Rc::new(VecSource { rows })));
        self.selection
            .set_selected(position.map_or(gtk4::INVALID_LIST_POSITION, |p| p as u32));
        self.quiet.set(false);
    }

    pub fn count(&self) -> usize {
        self.rows.borrow().len()
    }

    /// Bring row `position` into view.
    fn scroll_to(&self, position: u32) {
        self.view
            .scroll_to(position, None, gtk4::ListScrollFlags::NONE, None);
    }

    /// Select the game with this name, if the list has it (for `--game`).
    pub fn select_name(&self, name: &str) -> bool {
        let position = self.rows.borrow().iter().position(|r| r.name == name);
        match position {
            Some(p) => {
                self.selection.set_selected(p as u32);
                self.scroll_to(p as u32);
                true
            }
            None => false,
        }
    }
}

// ---- the ROM list ----------------------------------------------------------------------------

pub struct RomGrid {
    scroll: gtk4::ScrolledWindow,
    list: VirtualList,
    len: Cell<usize>,
    merge: gtk4::ColumnViewColumn,
    alt: Vec<gtk4::ColumnViewColumn>,
    status: gtk4::ColumnViewColumn,
    modified: gtk4::ColumnViewColumn,
}

impl RomGrid {
    pub fn new() -> RomGrid {
        let list = VirtualList::new();
        let selection = gtk4::NoSelection::new(Some(list.clone()));
        let view = gtk4::ColumnView::new(Some(selection));
        view.set_show_column_separators(true);

        // Got: the picture of where the DAT puts the ROM and what became of it.
        let got_factory = gtk4::SignalListItemFactory::new();
        got_factory.connect_setup(|_, obj| {
            let li = obj.downcast_ref::<gtk4::ListItem>().expect("a list item");
            let p = Painted::new(GOT_SIZE.0, GOT_SIZE.1);
            p.set_halign(gtk4::Align::Start);
            li.set_child(Some(&p));
        });
        got_factory.connect_bind(|_, obj| {
            let li = obj.downcast_ref::<gtk4::ListItem>().expect("a list item");
            let p = li.child().and_downcast::<Painted>().expect("a picture");
            if let Some(row) = li.item().and_then(|i| row_of::<RomRow>(&i)) {
                p.set_painter(Box::new(move |cr, _, _| {
                    draw_got(cr, row.dat_status, row.rep_status, row.icon_suffix)
                }));
            }
        });
        let got = gtk4::ColumnViewColumn::new(Some("Got"), Some(got_factory));
        got.set_fixed_width(64);
        view.append_column(&got);

        type C = gtk4::ColumnViewColumn;
        let col = |title: &str, width: i32, expand: bool, text: fn(&RomRow) -> String| -> C {
            text_column::<RomRow>(title, width, expand, text, |r| Some(r.rep_status))
        };
        let rom = col("ROM (File)", 340, true, |r| r.name.clone());
        let merge = col("Merge", 160, false, |r| r.merge.clone());
        let size = col("Size", 100, false, |r| r.size.clone());
        let crc = col("CRC32", 110, false, |r| r.crc.clone());
        let sha1 = col("SHA1", 290, false, |r| r.sha1.clone());
        let md5 = col("MD5", 240, false, |r| r.md5.clone());
        let alt_size = col("AltSize", 100, false, |r| r.alt_size.clone());
        let alt_crc = col("AltCRC32", 110, false, |r| r.alt_crc.clone());
        let alt_sha1 = col("AltSHA1", 290, false, |r| r.alt_sha1.clone());
        let alt_md5 = col("AltMD5", 240, false, |r| r.alt_md5.clone());
        let status = col("Status", 90, false, |r| r.status.clone());
        let modified = col("Modified Date/Time", 160, false, |r| r.modified.clone());
        let zip_index = col("ZipIndex", 70, false, |r| r.zip_index.clone());
        for c in [
            &rom, &merge, &size, &crc, &sha1, &md5, &alt_size, &alt_crc, &alt_sha1, &alt_md5,
            &status, &modified, &zip_index,
        ] {
            view.append_column(c);
        }

        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_child(Some(&view));
        scroll.set_hexpand(true);
        scroll.set_vexpand(true);
        RomGrid {
            scroll,
            list,
            len: Cell::new(0),
            merge,
            alt: vec![alt_size, alt_crc, alt_sha1, alt_md5],
            status,
            modified,
        }
    }

    pub fn widget(&self) -> &gtk4::ScrolledWindow {
        &self.scroll
    }

    pub fn count(&self) -> usize {
        self.len.get()
    }

    pub fn set_list(&self, list: RomList) {
        self.merge.set_visible(list.show_merge);
        for c in &self.alt {
            c.set_visible(list.show_alt);
        }
        self.status.set_visible(list.show_status);
        self.modified.set_visible(list.show_modified);
        self.len.set(list.rows.len());
        self.list
            .set_source(Some(Rc::new(VecSource { rows: list.rows })));
    }

    pub fn clear(&self) {
        self.len.set(0);
        self.list.set_source(None);
    }
}

impl Default for RomGrid {
    fn default() -> Self {
        RomGrid::new()
    }
}
