//! A `GListModel` that makes its items when the view asks for them.
//!
//! A directory with tens of thousands of games would otherwise mean tens of thousands of
//! `GObject`s built up front, on the UI thread, every time a tree row is selected. The view only
//! asks for the rows it shows, so only those are made. The rows themselves are plain Rust data in
//! a [`RowSource`]; the items handed to the view are [`glib::BoxedAnyObject`]s wrapping one row.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

/// Where the rows come from.
pub trait RowSource {
    fn len(&self) -> u32;
    /// The item for row `position` (`position < len()`).
    fn make(&self, position: u32) -> glib::Object;
}

/// Items kept so that asking twice for a row gives the same object; the view expects that. The
/// map is dropped when it grows past this many rows, which only costs a rebuild of what is shown.
const KEEP: usize = 2048;

mod imp {
    use super::*;
    use gtk4::gio;
    use gtk4::gio::subclass::prelude::ListModelImpl;

    #[derive(Default)]
    pub struct VirtualList {
        pub source: RefCell<Option<Rc<dyn RowSource>>>,
        pub made: RefCell<HashMap<u32, glib::Object>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VirtualList {
        const NAME: &'static str = "RvVirtualList";
        type Type = super::VirtualList;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for VirtualList {}

    impl ListModelImpl for VirtualList {
        fn item_type(&self) -> glib::Type {
            glib::BoxedAnyObject::static_type()
        }

        fn n_items(&self) -> u32 {
            self.source.borrow().as_ref().map_or(0, |s| s.len())
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let source = self.source.borrow().clone()?;
            if position >= source.len() {
                return None;
            }
            let mut made = self.made.borrow_mut();
            if let Some(item) = made.get(&position) {
                return Some(item.clone());
            }
            if made.len() >= KEEP {
                made.clear();
            }
            let item = source.make(position);
            made.insert(position, item.clone());
            Some(item)
        }
    }
}

glib::wrapper! {
    pub struct VirtualList(ObjectSubclass<imp::VirtualList>)
        @implements gtk4::gio::ListModel;
}

impl Default for VirtualList {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl VirtualList {
    pub fn new() -> VirtualList {
        VirtualList::default()
    }

    /// Replace the rows. The view is told once: everything old removed, everything new added.
    pub fn set_source(&self, source: Option<Rc<dyn RowSource>>) {
        let imp = self.imp();
        let old = imp.source.borrow().as_ref().map_or(0, |s| s.len());
        imp.made.borrow_mut().clear();
        let new = source.as_ref().map_or(0, |s| s.len());
        *imp.source.borrow_mut() = source;
        self.items_changed(0, old, new);
    }
}

/// A source over a `Vec` of rows that makes a [`glib::BoxedAnyObject`] holding a clone of a row.
pub struct VecSource<T: Clone + 'static> {
    pub rows: Vec<T>,
}

impl<T: Clone + 'static> RowSource for VecSource<T> {
    fn len(&self) -> u32 {
        self.rows.len() as u32
    }

    fn make(&self, position: u32) -> glib::Object {
        glib::BoxedAnyObject::new(self.rows[position as usize].clone()).upcast()
    }
}

/// The row of an item made by [`VecSource`].
pub fn row_of<T: Clone + 'static>(item: &glib::Object) -> Option<T> {
    item.downcast_ref::<glib::BoxedAnyObject>()
        .map(|b| b.borrow::<T>().clone())
}
