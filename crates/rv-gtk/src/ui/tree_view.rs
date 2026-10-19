//! The DAT tree: `ListView` + `TreeListModel` + `TreeExpander`, so that only the rows on screen
//! have widgets and a folder's children are made when it is opened.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gtk4::cairo::Context;
use gtk4::glib::{self, clone};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, gio, pango};
use rv_core::{ScanLevel, TreeSelect};

use super::icons::{draw_folder, FOLDER_SIZE};
use super::painted::Painted;
use crate::model::DirNode;

mod item {
    use super::*;
    use std::cell::Cell;

    mod imp {
        use super::*;

        #[derive(glib::Properties, Default)]
        #[properties(wrapper_type = super::TreeItem)]
        pub struct TreeItem {
            /// The tick box, live: it changes when a parent row is ticked.
            #[property(get, set)]
            pub checked: Cell<u32>,
            pub node: RefCell<Option<Arc<DirNode>>>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for TreeItem {
            const NAME: &'static str = "RvTreeItem";
            type Type = super::TreeItem;
        }

        #[glib::derived_properties]
        impl ObjectImpl for TreeItem {}
    }

    glib::wrapper! {
        pub struct TreeItem(ObjectSubclass<imp::TreeItem>);
    }

    impl TreeItem {
        pub fn new(node: Arc<DirNode>) -> TreeItem {
            let item: TreeItem = glib::Object::new();
            item.set_checked(u32::from(node.checked().0));
            *item.imp().node.borrow_mut() = Some(node);
            item
        }

        pub fn node(&self) -> Arc<DirNode> {
            self.imp().node.borrow().clone().expect("a tree item has a node")
        }
    }
}
use item::TreeItem;

/// What the tree tells the window.
#[derive(Clone, Debug)]
pub enum TreeEvent {
    Selected(Vec<String>),
    /// A tick box was clicked. `just_this` is Shift: do not tick the rows below.
    Tick {
        path: Vec<String>,
        value: TreeSelect,
        just_this: bool,
    },
    /// "Scan this directory" from the context menu.
    ScanDir { path: Vec<String>, level: ScanLevel },
}

type Registry = Rc<RefCell<HashMap<String, glib::WeakRef<TreeItem>>>>;

pub struct TreeView {
    scroll: gtk4::ScrolledWindow,
    selection: gtk4::SingleSelection,
    model: RefCell<Option<gtk4::TreeListModel>>,
    live: Registry,
    /// Selecting by code must not look like a click.
    quiet: Rc<std::cell::Cell<bool>>,
}

fn key(path: &[String]) -> String {
    path.join("/")
}

fn draw_tick(cr: &Context, value: TreeSelect) {
    let (x, y, s) = (1.0, 1.0, 12.0);
    cr.set_line_width(1.0);
    cr.rectangle(x + 0.5, y + 0.5, s, s);
    if value == TreeSelect::LOCKED {
        cr.set_source_rgb(0.85, 0.85, 0.85);
    } else {
        cr.set_source_rgb(1.0, 1.0, 1.0);
    }
    let _ = cr.fill_preserve();
    cr.set_source_rgb(0.35, 0.35, 0.35);
    let _ = cr.stroke();
    cr.set_line_width(1.8);
    cr.set_source_rgb(0.1, 0.1, 0.1);
    if value == TreeSelect::SELECTED {
        cr.move_to(x + 2.5, y + 7.0);
        cr.line_to(x + 5.5, y + 10.0);
        cr.line_to(x + 10.5, y + 3.5);
        let _ = cr.stroke();
    } else if value == TreeSelect::LOCKED {
        // A padlock: a body and a shackle.
        cr.rectangle(x + 3.0, y + 6.0, 7.0, 5.0);
        let _ = cr.fill();
        cr.set_line_width(1.4);
        cr.arc(x + 6.5, y + 6.0, 2.4, std::f64::consts::PI, std::f64::consts::TAU);
        let _ = cr.stroke();
    }
}

impl TreeView {
    pub fn new(events: impl Fn(TreeEvent) + Clone + 'static) -> TreeView {
        let live: Registry = Rc::default();
        let factory = gtk4::SignalListItemFactory::new();

        factory.connect_setup(clone!(
            #[strong]
            events,
            move |_, obj| {
                let li = obj.downcast_ref::<gtk4::ListItem>().expect("a list item");
                let expander = gtk4::TreeExpander::new();
                let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
                let tick = Painted::new(14, 14);
                let icon = Painted::new(FOLDER_SIZE, FOLDER_SIZE);
                let label = gtk4::Label::new(None);
                label.set_xalign(0.0);
                label.set_ellipsize(pango::EllipsizeMode::End);
                let counts = gtk4::Label::new(None);
                counts.set_xalign(0.0);
                counts.add_css_class("dim-label");
                row.append(&tick);
                row.append(&icon);
                row.append(&label);
                row.append(&counts);
                expander.set_child(Some(&row));
                li.set_child(Some(&expander));

                // The tick box: left click ticks or unticks, right click locks.
                let click = gtk4::GestureClick::new();
                click.set_button(0);
                click.connect_pressed(clone!(
                    #[strong]
                    events,
                    #[weak]
                    li,
                    move |g, _, _, _| {
                        let Some(item) = tree_item_of(&li) else { return };
                        g.set_state(gtk4::EventSequenceState::Claimed);
                        let node = item.node();
                        let current = TreeSelect(item.checked() as u8);
                        let value = if g.current_button() == gdk::BUTTON_SECONDARY {
                            if node.is_to_sort_primary_or_cache {
                                return;
                            }
                            TreeSelect::LOCKED
                        } else if current == TreeSelect::SELECTED {
                            TreeSelect::UN_SELECTED
                        } else {
                            TreeSelect::SELECTED
                        };
                        events(TreeEvent::Tick {
                            path: node.path.clone(),
                            value,
                            just_this: g.current_event_state().contains(gdk::ModifierType::SHIFT_MASK),
                        });
                    }
                ));
                tick.add_controller(click);

                // The rest of the row: right click for the scan menu.
                let menu = gtk4::GestureClick::new();
                menu.set_button(gdk::BUTTON_SECONDARY);
                menu.connect_pressed(clone!(
                    #[strong]
                    events,
                    #[weak]
                    li,
                    #[weak]
                    row,
                    move |_, _, x, y| {
                        let Some(item) = tree_item_of(&li) else { return };
                        scan_menu(&row, x, y, item.node().path.clone(), events.clone());
                    }
                ));
                row.add_controller(menu);
            }
        ));

        factory.connect_bind(|_, obj| {
            let li = obj.downcast_ref::<gtk4::ListItem>().expect("a list item");
            let Some(item) = tree_item_of(li) else { return };
            let node = item.node();
            let expander = li.child().and_downcast::<gtk4::TreeExpander>().expect("an expander");
            expander.set_list_row(li.item().and_downcast_ref::<gtk4::TreeListRow>());
            let row = expander.child().and_downcast::<gtk4::Box>().expect("a row");
            let tick = row.first_child().and_downcast::<Painted>().expect("a tick");
            let icon = tick.next_sibling().and_downcast::<Painted>().expect("an icon");
            let label = icon.next_sibling().and_downcast::<gtk4::Label>().expect("a label");
            let counts = label.next_sibling().and_downcast::<gtk4::Label>().expect("counts");
            label.set_text(&node.label);
            counts.set_text(&node.counts);
            if node.dats.is_empty() {
                row.set_tooltip_text(None);
            } else {
                row.set_tooltip_text(Some(&node.dats.join("\n")));
            }
            let (kind, shape) = (node.kind, node.icon);
            icon.set_painter(Box::new(move |cr, _, _| draw_folder(cr, shape, kind)));
            let weak = item.downgrade();
            tick.set_painter(Box::new(move |cr, _, _| {
                if let Some(item) = weak.upgrade() {
                    draw_tick(cr, TreeSelect(item.checked() as u8));
                }
            }));
            tick.track(&item, "checked");
        });

        factory.connect_unbind(|_, obj| {
            let li = obj.downcast_ref::<gtk4::ListItem>().expect("a list item");
            if let Some(expander) = li.child().and_downcast::<gtk4::TreeExpander>() {
                if let Some(tick) = expander
                    .child()
                    .and_downcast::<gtk4::Box>()
                    .and_then(|b| b.first_child())
                    .and_downcast::<Painted>()
                {
                    tick.untrack();
                }
                expander.set_list_row(None);
            }
        });

        let selection = gtk4::SingleSelection::new(None::<gio::ListModel>);
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        let view = gtk4::ListView::new(Some(selection.clone()), Some(factory));
        view.set_show_separators(false);
        view.add_css_class("rv-tree");
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_child(Some(&view));
        scroll.set_hexpand(true);
        scroll.set_vexpand(true);

        let quiet = Rc::new(std::cell::Cell::new(false));
        selection.connect_selected_item_notify(clone!(
            #[strong]
            events,
            #[strong]
            quiet,
            move |sel| {
                if quiet.get() {
                    return;
                }
                if let Some(item) = sel
                    .selected_item()
                    .and_downcast::<gtk4::TreeListRow>()
                    .and_then(|r| r.item())
                    .and_downcast::<TreeItem>()
                {
                    events(TreeEvent::Selected(item.node().path.clone()));
                }
            }
        ));

        TreeView {
            scroll,
            selection,
            model: RefCell::new(None),
            live,
            quiet,
        }
    }

    pub fn widget(&self) -> &gtk4::ScrolledWindow {
        &self.scroll
    }

    /// Show `rows`, open the ones that were open, and select `select` again if it is still there.
    pub fn set_rows(&self, rows: &[Arc<DirNode>], select: Option<&[String]>) {
        self.live.borrow_mut().clear();
        let top = gio::ListStore::new::<TreeItem>();
        let live = self.live.clone();
        let make = move |node: &Arc<DirNode>| {
            let item = TreeItem::new(node.clone());
            live.borrow_mut().insert(key(&node.path), item.downgrade());
            item
        };
        let top_items: Vec<TreeItem> = rows.iter().map(&make).collect();
        top.extend_from_slice(&top_items);

        let model = gtk4::TreeListModel::new(top, false, false, move |obj| {
            let item = obj.downcast_ref::<TreeItem>()?;
            let node = item.node();
            if node.children.is_empty() {
                return None;
            }
            let store = gio::ListStore::new::<TreeItem>();
            let kids: Vec<TreeItem> = node.children.iter().map(&make).collect();
            store.extend_from_slice(&kids);
            Some(store.upcast())
        });

        self.quiet.set(true);
        self.selection.set_model(Some(&model));
        // Open what was open: every row inserted by an opening is visited by the same loop.
        let mut i = 0;
        while i < model.n_items() {
            if let Some(row) = model.row(i) {
                let open = row
                    .item()
                    .and_downcast::<TreeItem>()
                    .is_some_and(|it| it.node().expanded());
                if open {
                    row.set_expanded(true);
                }
            }
            i += 1;
        }
        let mut selected = gtk4::INVALID_LIST_POSITION;
        if let Some(path) = select {
            for i in 0..model.n_items() {
                let found = model
                    .row(i)
                    .and_then(|r| r.item())
                    .and_downcast::<TreeItem>()
                    .is_some_and(|it| it.node().path == path);
                if found {
                    selected = i;
                    break;
                }
            }
        }
        self.selection.set_selected(selected);
        self.quiet.set(false);
        *self.model.borrow_mut() = Some(model);
    }

    /// Show a tick on the row at `path` and, unless `just_this`, on every row below it.
    pub fn apply_checked(&self, node: &DirNode, value: TreeSelect, just_this: bool) {
        node.set_checked(value);
        if let Some(item) = self.live.borrow().get(&key(&node.path)).and_then(|w| w.upgrade()) {
            item.set_checked(u32::from(value.0));
        }
        if just_this {
            return;
        }
        for child in &node.children {
            self.apply_checked(child, value, false);
        }
    }

    /// Which of the rows on screen are open, for the cache to remember.
    pub fn open_state(&self) -> Vec<(Vec<String>, bool)> {
        let model = self.model.borrow();
        let Some(model) = model.as_ref() else {
            return Vec::new();
        };
        (0..model.n_items())
            .filter_map(|i| {
                let row = model.row(i)?;
                let item = row.item().and_downcast::<TreeItem>()?;
                let node = item.node();
                (!node.children.is_empty()).then(|| (node.path.clone(), row.is_expanded()))
            })
            .collect()
    }
}

fn tree_item_of(li: &gtk4::ListItem) -> Option<TreeItem> {
    li.item()
        .and_downcast::<gtk4::TreeListRow>()
        .and_then(|r| r.item())
        .and_downcast::<TreeItem>()
}

fn scan_menu(
    parent: &gtk4::Box,
    x: f64,
    y: f64,
    path: Vec<String>,
    events: impl Fn(TreeEvent) + Clone + 'static,
) {
    let popover = gtk4::Popover::new();
    popover.set_parent(parent);
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    let list = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    for (text, level) in [
        ("Scan quick (headers only)", ScanLevel::Level1),
        ("Scan", ScanLevel::Level2),
        ("Scan full (complete re-scan)", ScanLevel::Level3),
    ] {
        let button = gtk4::Button::with_label(text);
        button.add_css_class("flat");
        button.connect_clicked(clone!(
            #[strong]
            events,
            #[strong]
            path,
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                events(TreeEvent::ScanDir {
                    path: path.clone(),
                    level,
                });
            }
        ));
        list.append(&button);
    }
    popover.set_child(Some(&list));
    popover.connect_closed(|p| {
        // The popover is made for each click; unparent it when it goes away.
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
}
