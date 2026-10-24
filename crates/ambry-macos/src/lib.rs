//! Native AppKit controls for GPUI windows.
//!
//! Each control is a real AppKit view added on top of GPUI's content view. GPUI keeps it in
//! its layout through [`NativeView::set_frame`], called from a GPUI element on every frame.

#[cfg(target_os = "macos")]
mod lab;
#[cfg(target_os = "macos")]
mod native;

#[cfg(target_os = "macos")]
pub use lab::open_switch_lab;
#[cfg(target_os = "macos")]
pub use native::{GlassButton, NativeView, TabSwitch};

/// Toolbar geometry in points, shared by the native and the GPUI-drawn toolbar.
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
