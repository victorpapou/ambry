//! Icon tabs drawn by NSTabView, which looks like Xcode's navigator and inspector tabs.
//! NSTabView only draws text labels, so each item draws an SF Symbol in place of its label.

use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
};
use objc2_app_kit::{
    NSColor, NSImage, NSImageSymbolConfiguration, NSTabPosition, NSTabState, NSTabView,
    NSTabViewBorderType, NSTabViewItem,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

/// Width given to each icon; AppKit adds its own padding around it.
const ICON_SLOT: f64 = 20.;

struct IconIvars {
    normal: Retained<NSImage>,
    selected: Retained<NSImage>,
}

define_class!(
    #[unsafe(super(NSTabViewItem))]
    #[name = "AmbryIconTabItem"]
    #[ivars = IconIvars]
    struct IconTabItem;

    impl IconTabItem {
        #[unsafe(method(sizeOfLabel:))]
        fn size_of_label(&self, _compute_min: Bool) -> NSSize {
            NSSize::new(ICON_SLOT, self.ivars().normal.size().height)
        }

        #[unsafe(method(drawLabel:inRect:))]
        fn draw_label(&self, _truncate: Bool, rect: NSRect) {
            let image = if self.tabState() == NSTabState::SelectedTab {
                &self.ivars().selected
            } else {
                &self.ivars().normal
            };
            let size = image.size();
            image.drawInRect(NSRect::new(
                NSPoint::new(
                    (rect.origin.x + (rect.size.width - size.width) / 2.).round(),
                    (rect.origin.y + (rect.size.height - size.height) / 2.).round(),
                ),
                size,
            ));
        }
    }
);

impl IconTabItem {
    fn new(symbol: &str, description: &str) -> Option<Retained<Self>> {
        let tinted = |color: &NSColor| {
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                Some(&NSString::from_str(description)),
            )?
            .imageWithSymbolConfiguration(
                &NSImageSymbolConfiguration::configurationWithHierarchicalColor(color),
            )
        };
        let ivars = IconIvars {
            normal: tinted(&NSColor::secondaryLabelColor())?,
            selected: tinted(&NSColor::labelColor())?,
        };
        let this = Self::alloc().set_ivars(ivars);
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.setLabel(&NSString::from_str(description));
        Some(this)
    }
}

/// A borderless tab strip with one icon tab per `(symbol, description)`, `height` points
/// tall and as wide as its tabs.
pub(crate) fn icon_tabs(
    mtm: MainThreadMarker,
    icons: &[(&str, &str)],
    height: f64,
) -> Option<Retained<NSTabView>> {
    let view = NSTabView::initWithFrame(
        NSTabView::alloc(mtm),
        NSRect::new(NSPoint::new(0., 0.), NSSize::new(200., height)),
    );
    view.setTabPosition(NSTabPosition::Top);
    view.setTabViewBorderType(NSTabViewBorderType::None);
    for (symbol, description) in icons {
        let item = IconTabItem::new(symbol, description)?;
        view.addTabViewItem(&item);
    }
    view.setFrameSize(NSSize::new(view.minimumSize().width, height));
    Some(view)
}
