//! The window's own NSToolbar with the canvas/list tabs and the add button, so macOS gives
//! them the toolbar's glass.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AllocAnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class,
    msg_send, sel,
};
use objc2_app_kit::{
    NSButton, NSImage, NSImageScaling, NSImageView, NSSegmentedControl, NSTabView, NSToolbar,
    NSToolbarDelegate, NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem,
    NSToolbarItemIdentifier, NSToolbarSpaceItemIdentifier, NSView, NSWindowToolbarStyle,
};
use objc2_foundation::{NSArray, NSCopying, NSPoint, NSRect, NSSize, NSString};

use crate::native::{
    Target, decorate, icon_images, icon_tab_view, object, segments, selected_index, symbol,
};

const MODES: &str = "modes";
const ADD: &str = "add";
const AVATAR: &str = "avatar";
/// Point size of the account picture inside its glass circle.
const AVATAR_SIZE: f64 = 28.;

struct DelegateIvars {
    /// The view of each item, made up front so the window can keep hold of them.
    views: Vec<(&'static str, Retained<NSView>)>,
    items: RefCell<Vec<Retained<NSToolbarItem>>>,
}

define_class!(
    // Hands the toolbar its items.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryWindowToolbarDelegate"]
    #[ivars = DelegateIvars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSToolbarDelegate for Delegate {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(
            &self,
            _toolbar: &NSToolbar,
            id: &NSToolbarItemIdentifier,
            _flag: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            self.make_item(id)
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_ids(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            ids()
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_ids(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            ids()
        }
    }
);

impl Delegate {
    fn make_item(&self, id: &NSToolbarItemIdentifier) -> Option<Retained<NSToolbarItem>> {
        let name = id.to_string();
        let (_, view) = self.ivars().views.iter().find(|(key, _)| *key == name)?;
        let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(self.mtm()), id);
        item.setView(Some(view));
        item.setLabel(&NSString::from_str(match name.as_str() {
            MODES => "View",
            ADD => "Add",
            _ => "Account",
        }));
        self.ivars().items.borrow_mut().push(item.clone());
        Some(item)
    }
}

/// The tabs, the add button and the account, each on its own glass: a space between items
/// keeps the toolbar from merging them into one platter.
fn ids() -> Retained<NSArray<NSToolbarItemIdentifier>> {
    let flexible: Retained<NSString> = unsafe { NSToolbarFlexibleSpaceItemIdentifier }.copy();
    let space: Retained<NSString> = unsafe { NSToolbarSpaceItemIdentifier }.copy();
    NSArray::from_retained_slice(&[
        flexible,
        NSString::from_str(MODES),
        space.clone(),
        NSString::from_str(ADD),
        space,
        NSString::from_str(AVATAR),
    ])
}

/// The toolbar of the window that holds `ns_view`: canvas and list tabs that lift a glass
/// bubble while pressed, and a button that adds files.
pub struct WindowToolbar {
    tabs: Retained<NSTabView>,
    delegate: Retained<Delegate>,
    _targets: [Retained<Target>; 2],
}

impl WindowToolbar {
    /// # Safety
    /// `ns_view` must point to a live `NSView` in a window, and this must run on the main
    /// thread.
    pub unsafe fn attach(
        ns_view: NonNull<c_void>,
        tabs: &[(&str, &str)],
        item_width: f64,
        avatar: Option<&Path>,
        initial: &str,
        on_mode: impl Fn(usize) + 'static,
        on_add: impl Fn() + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let view: &NSView = unsafe { ns_view.cast().as_ref() };
        let window = view.window()?;

        let images = icon_images(tabs)?;
        let tab_view = icon_tab_view(mtm, tabs.len(), &images, item_width);
        tab_view.setDrawsBackground(false);
        let tabs_target = Target::with_tabs(
            mtm,
            move |sender| {
                let Some(tabs) = sender.downcast_ref::<NSTabView>() else {
                    return;
                };
                decorate(tabs, &images, item_width);
                if let Some(index) = selected_index(tabs) {
                    on_mode(index);
                }
            },
            |_, _| true,
        );
        tab_view.setDelegate(Some(ProtocolObject::from_ref(&*tabs_target)));
        let strip = cut_to_strip(mtm, &tab_view);

        let add_target = Target::new(mtm, move |_| on_add());
        let image = symbol("plus", "Add files or folders")?;
        let add = unsafe {
            NSButton::buttonWithImage_target_action(
                &image,
                Some(&add_target),
                Some(sel!(fire:)),
                mtm,
            )
        };
        add.setToolTip(Some(&NSString::from_str("Add files or folders")));

        let delegate: Retained<Delegate> = {
            let this = Delegate::alloc(mtm).set_ivars(DelegateIvars {
                views: vec![
                    (MODES, strip),
                    (ADD, Retained::into_super(Retained::into_super(add))),
                    (AVATAR, account(mtm, avatar, initial)),
                ],
                items: RefCell::new(Vec::new()),
            });
            unsafe { msg_send![super(this), init] }
        };
        let toolbar = NSToolbar::initWithIdentifier(
            NSToolbar::alloc(mtm),
            &NSString::from_str("ambry.window"),
        );
        toolbar.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        toolbar.setAllowsUserCustomization(false);
        window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        window.setToolbar(Some(&toolbar));
        Some(Self {
            tabs: tab_view,
            delegate,
            _targets: [tabs_target, add_target],
        })
    }

    /// Selects tab `index` without reporting it.
    pub fn select(&self, index: usize) {
        if selected_index(&self.tabs) != Some(index) {
            self.tabs.selectTabViewItemAtIndex(index as isize);
        }
    }

    /// The toolbar's views and items, by name, for [`crate::probe::run`].
    pub fn probe_targets(&self) -> Vec<(String, Retained<AnyObject>)> {
        let mut targets = vec![("tabs".to_string(), object(self.tabs.clone()))];
        if let Some(control) = segments(&self.tabs) {
            targets.push(("control".into(), object(control)));
        }
        for item in self.delegate.ivars().items.borrow().iter() {
            let name = item.itemIdentifier().to_string();
            targets.push((format!("item-{name}"), object(item.clone())));
        }
        // The views AppKit wraps around the strip: item viewer, platter, glass.
        let mut view = unsafe { self.tabs.superview() };
        let mut depth = 0;
        while let Some(current) = view {
            let class = current.class().name().to_string_lossy().into_owned();
            let name = if class.contains("Platter") {
                "platter".to_string()
            } else if class.contains("ItemViewer") {
                "viewer".to_string()
            } else if class.contains("GlassEffectView") && !class.contains("Content") {
                "glass".to_string()
            } else if class.contains("ToolbarView") && !class.contains("Item") {
                "toolbar-root".to_string()
            } else {
                format!("up{depth}")
            };
            targets.push((name, object(current.clone())));
            view = unsafe { current.superview() };
            depth += 1;
            if depth > 8 {
                break;
            }
        }
        targets
    }
}

/// The account picture cut to a circle, or the user's initial as a button, for the toolbar
/// to put on glass.
fn account(mtm: MainThreadMarker, picture: Option<&Path>, initial: &str) -> Retained<NSView> {
    let image = picture.and_then(|path| {
        NSImage::initWithContentsOfFile(
            NSImage::alloc(),
            &NSString::from_str(&path.to_string_lossy()),
        )
    });
    match image {
        Some(image) => {
            let view = NSImageView::imageViewWithImage(&image, mtm);
            view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            view.setFrame(NSRect::new(
                NSPoint::new(0., 0.),
                NSSize::new(AVATAR_SIZE, AVATAR_SIZE),
            ));
            view.setWantsLayer(true);
            if let Some(layer) = view.layer() {
                layer.setCornerRadius(AVATAR_SIZE / 2.);
                layer.setMasksToBounds(true);
            }
            Retained::into_super(Retained::into_super(view))
        }
        None => {
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(initial),
                    None,
                    None,
                    mtm,
                )
            };
            Retained::into_super(Retained::into_super(button))
        }
    }
}

/// A view exactly the size of the tab strip with the tab view inside it, so the toolbar sizes
/// the item to the strip and not to NSTabView's margins.
fn cut_to_strip(mtm: MainThreadMarker, tabs: &Retained<NSTabView>) -> Retained<NSView> {
    let Some(control) = segments(tabs) else {
        return Retained::into_super(tabs.clone());
    };
    let strip = control.frame();
    // NSTabView keeps a few views of its own around the strip; only the strip should show.
    for view in tabs.subviews().iter() {
        if !view.isKindOfClass(NSSegmentedControl::class()) {
            view.setHidden(true);
        }
    }
    tabs.setFrameOrigin(NSPoint::new(-strip.origin.x, -strip.origin.y));
    let holder = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0., 0.), strip.size),
    );
    holder.addSubview(tabs);
    holder
}
