//! Native AppKit controls for GPUI windows.
//!
//! Each control is a real AppKit view added on top of GPUI's content view. GPUI keeps it in
//! its layout through [`NativeView::set_frame`], called from a GPUI element on every frame.

#[cfg(target_os = "macos")]
mod box_list;
#[cfg(target_os = "macos")]
mod columns;
#[cfg(target_os = "macos")]
pub mod finder;
#[cfg(target_os = "macos")]
mod icon_picker;
#[cfg(target_os = "macos")]
mod lab;
#[cfg(target_os = "macos")]
mod lab_toolbar;
#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
pub mod probe;
#[cfg(target_os = "macos")]
pub mod quick_look;
#[cfg(target_os = "macos")]
mod rename;
#[cfg(target_os = "macos")]
mod scroller;
#[cfg(target_os = "macos")]
pub mod settings;
#[cfg(target_os = "macos")]
mod soft_edge;
#[cfg(target_os = "macos")]
mod table;
#[cfg(target_os = "macos")]
mod tilt;
#[cfg(target_os = "macos")]
mod toolbar;
#[cfg(target_os = "macos")]
mod zoom;
#[cfg(target_os = "macos")]
mod zoom_label;

#[cfg(target_os = "macos")]
#[cfg(target_os = "macos")]
pub use columns::{ColumnBrowser, ColumnEvent};
#[cfg(target_os = "macos")]
pub use lab::open_switch_lab;
#[cfg(target_os = "macos")]
pub use native::{
    GlassAvatar, GlassButton, GlassSegments, GlassTabs, NativeView, TabEvent, TabSwitch,
    set_window_subtitle,
};
#[cfg(target_os = "macos")]
pub use scroller::CanvasScrollView;
#[cfg(target_os = "macos")]
pub use soft_edge::SoftEdge;
#[cfg(target_os = "macos")]
pub use table::{FileRow, FileTable};
#[cfg(target_os = "macos")]
pub use toolbar::{BoxLabel, ToolbarEvent, WindowToolbar};
#[cfg(target_os = "macos")]
pub use zoom::PinchZoom;
#[cfg(target_os = "macos")]
pub use zoom_label::ZoomLabel;

/// Titlebar geometry in points, shared by the native and the GPUI-drawn controls.
pub mod layout {
    /// Distance of the controls from the window's top and right edges.
    pub const TOP: f64 = 12.;
    pub const HEIGHT: f64 = 30.;
    pub const AVATAR: f64 = 34.;
    pub const GAP_BEFORE_AVATAR: f64 = 16.;
    /// Height of the band the controls sit in; the list starts under it.
    pub const BAR: f64 = 54.;
    /// The shade behind the toolbar: its solid half covers the toolbar, the rest fades out
    /// toward the content.
    pub const TOOLBAR_BLUR: f64 = 108.;
    /// The add button, and its distance from the window's bottom and right edges.
    pub const ADD: f64 = 36.;
    pub const ADD_MARGIN: f64 = 14.;
    /// Where the title starts, right of the traffic lights, and the height it is centred in.
    pub const TITLE_LEFT: f64 = 80.;
    pub const TITLE_HEIGHT: f64 = 38.;
    /// Width of the GPUI-drawn switch used where AppKit is not available.
    pub const SEGMENTS_WIDTH: f64 = 70.;
    pub const GAP: f64 = 8.;
}
