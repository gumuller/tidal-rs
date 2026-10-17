use gtk::{gdk, glib, graphene, prelude::*, subclass::prelude::*};
use std::cell::{Cell, RefCell};

mod imp {
    use super::*;
    #[derive(Default)]
    pub struct Artwork {
        pub size: Cell<i32>,
        pub texture: RefCell<Option<gdk::Texture>>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for Artwork {
        const NAME: &'static str = "TidalArtwork";
        type Type = super::Artwork;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for Artwork {}
    impl WidgetImpl for Artwork {
        // Texture resolution must never contribute to the layout. A 320px
        // download stays 164px in the grid and 42/48px in rows/the player bar.
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (self.size.get(), self.size.get(), -1, -1)
        }
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(texture) = self.texture.borrow().as_ref() {
                snapshot.append_texture(
                    texture,
                    &graphene::Rect::new(
                        0.0,
                        0.0,
                        self.obj().width() as f32,
                        self.obj().height() as f32,
                    ),
                );
            }
        }
    }
}

glib::wrapper! {
    pub struct Artwork(ObjectSubclass<imp::Artwork>)
        @extends gtk::Widget,
        @implements gtk::Accessible,gtk::Buildable,gtk::ConstraintTarget;
}

impl Artwork {
    pub fn new(size: i32) -> Self {
        let widget: Self = glib::Object::new();
        widget.imp().size.set(size);
        widget.add_css_class("cover");
        widget
    }
    pub fn set_texture(&self, texture: &gdk::Texture) {
        self.imp().texture.replace(Some(texture.clone()));
        self.queue_draw();
    }
}
