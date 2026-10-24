//! Native AppKit pieces that GPUI does not draw itself.
//!
//! Right now that is the floating toolbar in the bottom-left corner of a box window:
//! a canvas/list switch on a Liquid Glass capsule and a glass "add" button.

#[cfg(target_os = "macos")]
mod glass_switch;
#[cfg(target_os = "macos")]
mod lab;
#[cfg(target_os = "macos")]
mod toolbar;

#[cfg(target_os = "macos")]
pub use lab::open_switch_lab;

#[cfg(target_os = "macos")]
pub use toolbar::Toolbar;

/// What the user did in the native toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarEvent {
    /// The canvas (0) or list (1) segment was selected.
    Mode(usize),
    Add,
}

/// Toolbar geometry in points, shared with the GPUI side so both halves line up.
pub mod layout {
    /// Distance from the window's left and bottom edges.
    pub const INSET: f64 = 20.;
    pub const HEIGHT: f64 = 36.;
    pub const AVATAR: f64 = 36.;
    pub const GAP_AFTER_AVATAR: f64 = 12.;
    /// Width of the GPUI-drawn switch used where AppKit is not available.
    pub const SEGMENTS_WIDTH: f64 = 70.;
    pub const GAP: f64 = 8.;
}
