//! `Painted`: a `DrawingArea` whose picture can be swapped, for the cells of a list view.
//!
//! A list view reuses its cell widgets for different rows, so a cell has to be told what to draw
//! each time it is bound. `set_painter` does that; `track` makes the cell redraw when a property
//! of the bound item changes (the tick box does, when a parent row is ticked).

use std::cell::RefCell;

use gtk4::cairo::Context;
use gtk4::glib::{self, SignalHandlerId};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

/// A picture: the context, the width and the height to draw in.
pub type Painter = Box<dyn Fn(&Context, i32, i32)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Painted {
        pub painter: RefCell<Option<Painter>>,
        pub tracked: RefCell<Option<(glib::WeakRef<glib::Object>, SignalHandlerId)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Painted {
        const NAME: &'static str = "RvPainted";
        type Type = super::Painted;
        type ParentType = gtk4::DrawingArea;
    }

    impl ObjectImpl for Painted {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_draw_func(|area, cr, w, h| {
                let area = area.downcast_ref::<super::Painted>().expect("a Painted");
                if let Some(paint) = area.imp().painter.borrow().as_ref() {
                    paint(cr, w, h);
                }
            });
        }
    }

    impl WidgetImpl for Painted {}
    impl DrawingAreaImpl for Painted {}
}

glib::wrapper! {
    pub struct Painted(ObjectSubclass<imp::Painted>)
        @extends gtk4::DrawingArea, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Painted {
    /// A picture area of a fixed size.
    pub fn new(width: i32, height: i32) -> Painted {
        let p: Painted = glib::Object::new();
        p.set_content_width(width);
        p.set_content_height(height);
        p
    }

    /// Draw `painter` from now on, and now.
    pub fn set_painter(&self, painter: Painter) {
        *self.imp().painter.borrow_mut() = Some(painter);
        self.queue_draw();
    }

    /// Redraw when `property` of `item` changes; forget the item tracked before.
    pub fn track(&self, item: &impl IsA<glib::Object>, property: &str) {
        self.untrack();
        let this = self.downgrade();
        let id = item.connect_notify_local(Some(property), move |_, _| {
            if let Some(area) = this.upgrade() {
                area.queue_draw();
            }
        });
        *self.imp().tracked.borrow_mut() =
            Some((item.upcast_ref::<glib::Object>().downgrade(), id));
    }

    /// Stop redrawing for the item tracked, if it is still there.
    pub fn untrack(&self) {
        if let Some((weak, id)) = self.imp().tracked.borrow_mut().take() {
            if let Some(item) = weak.upgrade() {
                item.disconnect(id);
            }
        }
    }
}
