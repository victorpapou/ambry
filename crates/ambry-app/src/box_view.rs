//! The box window: a dotted canvas of files drawn like Finder's icon view, a list view and the
//! bottom bar. On macOS the list, the avatar and the bottom bar controls are AppKit views.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(target_os = "macos")]
use ambry_core::BoxIcon;
use ambry_core::{BoxDoc, BoxEntry, DROP_STEP_X, Item, ItemKind, Store};
use ambry_macos::layout;
use gpui::{
    Animation, AnimationExt as _, App, Bounds, BoxShadow, Context, CursorStyle, DragMoveEvent,
    ExternalPaths, FocusHandle, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    ObjectFit, Pixels, Point, Rgba, ScrollWheelEvent, SharedString, Window, WindowAppearance,
    canvas, div, ease_out_quint, fill, hsla, img, linear_color_stop, linear_gradient, point,
    prelude::*, px, rgb, size, svg,
};

#[cfg(target_os = "macos")]
use crate::native_view::native;

/// Finder icons on the canvas are this many points wide.
const ICON_SIZE: f32 = 64.;
const DOT_SPACING: f32 = 25.;
const DOT_RADIUS: f32 = 1.25;
const CARD_WIDTH: f32 = 64.;
const CARD_HEIGHT: f32 = 80.;
/// Width of a card together with its file name underneath.
const ITEM_WIDTH: f32 = 128.;
/// Height of a card together with its file name underneath.
const ITEM_HEIGHT: f32 = CARD_HEIGHT + 40.;
/// Width of the white fade along the canvas edges.
const EDGE_FADE: f32 = 40.;
/// Empty canvas one can scroll to around the items, in points.
const CANVAS_MARGIN: f32 = 400.;
const TITLE_SIZE: f32 = 15.;
const LABEL_SIZE: f32 = 13.;
const ACCENT: u32 = 0x2f7cf6;

/// How far the canvas zooms, and how much a key press or a click on a scroll wheel step zooms.
/// How hard a pinch past the zoom limits pulls: the zoom shown grows as this power of the
/// zoom asked for.
const RUBBER: f32 = 0.3;
/// How much bigger a dragged item is drawn.
const LIFT: f32 = 0.06;
/// How long a folder takes to open under a drag, and to close after it.
const OPEN_FADE_MS: u64 = 160;
/// How long the in-between dots take to switch on or off, in seconds.
const DOT_FADE: f32 = 0.18;
/// How long the canvas takes to spring back to an edge, in seconds.
const SPRING_RESPONSE: f32 = 0.4;
const MIN_ZOOM: f32 = 0.3;
const MAX_ZOOM: f32 = 4.;
const ZOOM_STEP: f32 = 1.25;

/// The window's colors in one appearance.
struct Palette {
    canvas: u32,
    ink: u32,
    quiet: u32,
    faint: u32,
    dot: u32,
    hairline: u32,
    pressed: u32,
    /// Top of a file card's gradient; the bottom is the canvas color.
    card_top: u32,
    /// Behind a selected icon, like Finder's.
    selection: Hsla,
    /// Behind every other row of the list, like Finder's alternating rows.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    stripe: u32,
}

const LIGHT: Palette = Palette {
    canvas: 0xffffff,
    ink: 0x1d1d1f,
    quiet: 0x8e8e93,
    faint: 0xb4b4ba,
    dot: 0xc7c7cc,
    hairline: 0xe5e5ea,
    pressed: 0xececf0,
    card_top: 0xf2f2f5,
    selection: Hsla {
        h: 0.,
        s: 0.,
        l: 0.,
        a: 0.08,
    },
    stripe: 0xf4f5f5,
};

const DARK: Palette = Palette {
    canvas: 0x1e1e1e,
    ink: 0xf5f5f7,
    quiet: 0x98989d,
    faint: 0x6e6e73,
    dot: 0x5c5c60,
    hairline: 0x3a3a3c,
    pressed: 0x2c2c2e,
    card_top: 0x2c2c2e,
    selection: Hsla {
        h: 0.,
        s: 0.,
        l: 1.,
        a: 0.12,
    },
    stripe: 0x2a2a2a,
};

thread_local! {
    static DARK_MODE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The palette for the window's current appearance, set at the start of every render.
fn palette() -> &'static Palette {
    if DARK_MODE.get() { &DARK } else { &LIGHT }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Canvas,
    Columns,
}

#[derive(Clone, Copy)]
struct Marquee {
    start: Point<Pixels>,
    now: Point<Pixels>,
}

/// How an item came to rest on the canvas, which sets its bounce.
#[derive(Clone, Copy)]
enum Landing {
    /// Just added: it opens out from small, like a new folder in Finder.
    Added,
    /// Let go after a drag: it drops a little below its size and settles.
    Dropped,
}

struct Drag {
    id: u64,
    /// Mouse position minus the item's top-left corner, in window coordinates.
    grab: Point<Pixels>,
    moved: bool,
    /// Where the mouse is now, in window coordinates.
    at: Point<Pixels>,
}

/// Files being dragged in from Finder, shown as ghost cards under the cursor.
struct DropPreview {
    position: Point<Pixels>,
    items: Vec<Item>,
}

pub struct BoxView {
    doc: BoxDoc,
    store: Store,
    /// Every saved box, for the box switcher.
    boxes: Vec<BoxEntry>,
    mode: Mode,
    /// Offset of the canvas origin from the top-left of the canvas area.
    pan: Point<Pixels>,
    /// How big the canvas is drawn, 1 for actual size.
    zoom: f32,
    /// Counts wheel scrolls, so a spring back to the edge knows it was overtaken.
    scroll_gen: u64,
    /// Whether fingers are on the trackpad, scrolling; whether a pull past an edge is
    /// springing back, and how fast.
    fingers: bool,
    springing: bool,
    pan_velocity: Point<Pixels>,
    /// The zoom a pinch under way asks for, before the resistance past the limits.
    pinch_raw: Option<f32>,
    selected: Option<u64>,
    /// Items picked by dragging a rectangle over them.
    picked: HashSet<u64>,
    /// The rectangle being dragged, from where the press landed to the pointer.
    marquee: Option<Marquee>,
    /// The contents of each folder picked in the column view.
    drag: Option<Drag>,
    drop_preview: Option<DropPreview>,
    /// Seconds to wait before a card plays its appear animation, so a batch lands one by one.
    appear_delay: HashMap<u64, f32>,
    /// When each item last landed, for its bounce, and when the dragged one was lifted.
    landed: HashMap<u64, (std::time::Instant, Landing)>,
    lifted_at: Option<std::time::Instant>,
    /// Whether the dots between every other one show, when that last switched, and how
    /// opaque they are now: they switch off as the grid closes in, with a short fade.
    dense_dots: (bool, std::time::Instant),
    between_dots: f32,
    /// Items that have been selected, whose highlight fades out when they are not.
    was_selected: std::cell::RefCell<HashSet<u64>>,
    /// The folder something is being dragged over, which opens like Finder's, and the folders
    /// that have opened, whose lid closes again when the drag moves on.
    open_folder: Option<u64>,
    was_open: std::cell::RefCell<HashSet<u64>>,
    /// Finder's open folder icon, rendered once into the data directory.
    open_icon: Option<PathBuf>,
    viewport: gpui::Size<Pixels>,
    avatar: Option<PathBuf>,
    focus: FocusHandle,
    /// Finder's icon for each item path, rendered once into the data directory.
    icons: HashMap<PathBuf, (Option<std::time::SystemTime>, Option<PathBuf>)>,
    /// Date modified, size and kind of each item path, as the list shows them.
    #[cfg(target_os = "macos")]
    details: HashMap<PathBuf, [String; 3]>,
    #[cfg(target_os = "macos")]
    controls: Option<NativeControls>,
}

/// The AppKit views of the window. GPUI lays each out through a `native()` element.
#[cfg(target_os = "macos")]
struct NativeControls {
    ns_view: std::ptr::NonNull<std::ffi::c_void>,
    toolbar: ambry_macos::WindowToolbar,
    /// What the toolbar shows, to touch AppKit only when it changes.
    shown_box: Option<(ambry_macos::BoxLabel, String)>,
    shown_boxes: (Vec<ambry_macos::BoxLabel>, Option<usize>),
    /// The blur behind the toolbar.
    toolbar_blur: ambry_macos::SoftEdge,
    /// AppKit's overlay scroll bars along the canvas's right and bottom edges.
    scroll: ambry_macos::CanvasScrollView,
    /// The add button, bottom right.
    add: ambry_macos::GlassButton,
    /// Pinching over the window zooms the canvas.
    _pinch: ambry_macos::PinchZoom,
    /// The zoom as a percentage, bottom left.
    zoom_label: ambry_macos::ZoomLabel,
    /// Finder's column view.
    columns: ambry_macos::ColumnBrowser,
    /// The knob proportions and values last shown, to touch AppKit only when they change.
    shown_scroll: Option<[f64; 4]>,
    /// Where Quick Look sends the item it moved to.
    events: async_channel::Sender<ControlEvent>,
}

#[cfg(target_os = "macos")]
enum ControlEvent {
    Mode(usize),
    Add,
    Color(usize),
    Symbol(String),
    OpenBox(usize),
    NewBox,
    RenameBox(usize, String),
    Columns(ambry_macos::ColumnEvent),
    /// A scroll bar moved the visible part of the canvas to this top-left, counted from the
    /// top-left of [`BoxView::scroll_area`].
    Scroll(f64, f64),
    /// Quick Look moved to the item at this index.
    Preview(usize),
    /// Fingers pinched by this fraction, at this position from the window's top-left.
    Zoom(f64, f64, f64),
    /// The zoom label was clicked: back to actual size.
    ZoomReset,
    Settings(ambry_macos::settings::SettingsEvent),
}

impl BoxView {
    pub fn new(doc: BoxDoc, store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        let appear_delay = doc
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| (item.id, index.min(12) as f32 * 0.035))
            .collect();
        let avatar = avatar_path().filter(|path| path.exists());
        let boxes = ambry_core::list_boxes(store.dir());
        #[cfg(target_os = "macos")]
        let boxes_dir = store.dir().to_path_buf();
        Self {
            doc,
            store,
            boxes,
            mode: default_mode(),
            pan: point(px(0.), px(0.)),
            zoom: 1.,
            scroll_gen: 0,
            pinch_raw: None,
            fingers: false,
            springing: false,
            pan_velocity: point(px(0.), px(0.)),
            selected: None,
            picked: HashSet::new(),
            marquee: None,
            drag: None,
            drop_preview: None,
            appear_delay,
            landed: HashMap::new(),
            lifted_at: None,
            dense_dots: (true, std::time::Instant::now() - Duration::from_secs(1)),
            between_dots: 1.,
            was_selected: Default::default(),
            open_folder: None,
            was_open: Default::default(),
            open_icon: open_folder_file(),
            viewport: window.viewport_size(),
            #[cfg(target_os = "macos")]
            controls: Self::attach_controls(boxes_dir, window, cx),
            avatar,
            focus,
            icons: HashMap::new(),
            #[cfg(target_os = "macos")]
            details: HashMap::new(),
        }
    }

    #[cfg(target_os = "macos")]
    fn attach_controls(
        boxes_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<NativeControls> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        let ns_view = appkit.ns_view;
        let (events, received) = async_channel::unbounded();
        // SAFETY: GPUI hands out its own live content view and we are on the main thread.
        let controls = unsafe {
            let scrolled = events.clone();
            let previewed = events.clone();
            let pinched = events.clone();
            let configured = events.clone();
            ambry_macos::settings::configure(
                avatar_path().unwrap_or_default(),
                boxes_dir,
                move |event| {
                    configured.try_send(ControlEvent::Settings(event)).ok();
                },
            );
            let toolbar_blur = ambry_macos::SoftEdge::new(ns_view, layout::TOOLBAR_BLUR)?;
            let scroll = ambry_macos::CanvasScrollView::new(ns_view, move |x, y| {
                scrolled.try_send(ControlEvent::Scroll(x, y)).ok();
            })?;
            let reset = events.clone();
            let toolbar = ambry_macos::WindowToolbar::attach(
                ns_view,
                &BoxIcon::COLORS,
                BoxIcon::SYMBOL_GROUPS,
                avatar_path().as_deref(),
                &user_initial(),
                move |event| {
                    let event = match event {
                        ambry_macos::ToolbarEvent::Mode(index) => ControlEvent::Mode(index),
                        ambry_macos::ToolbarEvent::Color(index) => ControlEvent::Color(index),
                        ambry_macos::ToolbarEvent::Symbol(name) => ControlEvent::Symbol(name),
                        ambry_macos::ToolbarEvent::OpenBox(index) => ControlEvent::OpenBox(index),
                        ambry_macos::ToolbarEvent::NewBox => ControlEvent::NewBox,
                        ambry_macos::ToolbarEvent::RenameBox(index, name) => {
                            ControlEvent::RenameBox(index, name)
                        }
                    };
                    events.try_send(event).ok();
                },
            )?;
            let pinch = ambry_macos::PinchZoom::new(ns_view, move |step, x, y| {
                pinched.try_send(ControlEvent::Zoom(step, x, y)).ok();
            })?;
            let browsed = previewed.clone();
            let columns = ambry_macos::ColumnBrowser::new(ns_view, move |event| {
                browsed.try_send(ControlEvent::Columns(event)).ok();
            })?;
            let zoom_label = ambry_macos::ZoomLabel::new(ns_view, move || {
                reset.try_send(ControlEvent::ZoomReset).ok();
            })?;
            let added = previewed.clone();
            let add = ambry_macos::GlassButton::new(
                ns_view,
                "plus",
                "Add files or folders",
                layout::ADD,
                move || {
                    added.try_send(ControlEvent::Add).ok();
                },
            )?;
            add.set_white();
            NativeControls {
                ns_view,
                toolbar,
                shown_box: None,
                shown_boxes: (Vec::new(), None),
                toolbar_blur,
                scroll,
                add,
                _pinch: pinch,
                zoom_label,
                columns,
                shown_scroll: None,
                events: previewed,
            }
        };
        // AMBRY_DUMP=<file> writes the window's views and their frames there once it settles.
        if let Some(file) = std::env::var_os("AMBRY_DUMP") {
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(3))
                    .await;
                // SAFETY: the window outlives this task's few seconds, on the main thread.
                let tree = unsafe { ambry_macos::probe::window_tree(ns_view) };
                std::fs::write(file, tree).ok();
            })
            .detach();
        }
        cx.spawn(async move |this, cx| {
            while let Ok(event) = received.recv().await {
                let handled = this.update(cx, |this, cx| match event {
                    ControlEvent::Mode(index) => this.set_mode(
                        if index == 0 {
                            Mode::Canvas
                        } else {
                            Mode::Columns
                        },
                        cx,
                    ),
                    ControlEvent::Add => this.pick_paths(cx),
                    ControlEvent::Columns(ambry_macos::ColumnEvent::Open(path)) => {
                        cx.open_with_system(&path)
                    }
                    ControlEvent::Columns(ambry_macos::ColumnEvent::Pick(rows)) => {
                        let ids: Vec<u64> = rows
                            .iter()
                            .filter_map(|row| this.doc.items.get(*row).map(|item| item.id))
                            .collect();
                        this.selected = ids.last().copied();
                        this.picked = ids.into_iter().collect();
                        this.preview(false);
                        cx.notify();
                    }
                    ControlEvent::Color(index) => {
                        if let Some(color) = BoxIcon::COLORS.get(index) {
                            this.doc.icon.color = color.to_string();
                            this.save();
                            cx.notify();
                        }
                    }
                    ControlEvent::Symbol(name) => {
                        this.doc.icon.symbol = name;
                        this.save();
                        cx.notify();
                    }
                    ControlEvent::RenameBox(index, name) => this.rename_box(index, name, cx),
                    ControlEvent::OpenBox(index) => {
                        if let Some(entry) = this.boxes.get(index).cloned() {
                            this.open_box(Store::new(entry.path), cx);
                        }
                    }
                    ControlEvent::NewBox => {
                        let name = this.new_box_name();
                        let store = Store::new(ambry_core::new_box_path(this.store.dir(), &name));
                        let doc = BoxDoc::new(name);
                        if let Err(err) = store.save(&doc) {
                            eprintln!("could not save {}: {err:#}", store.path().display());
                        }
                        this.open_box(store, cx);
                    }
                    ControlEvent::Scroll(x, y) => this.on_scroller(x, y, cx),
                    ControlEvent::ZoomReset => {
                        let centre = this.viewport_centre();
                        this.zoom_at(1. / this.zoom, centre, cx)
                    }
                    ControlEvent::Zoom(step, x, y) => {
                        this.pinch(step as f32, point(px(x as f32), px(y as f32)), cx)
                    }
                    ControlEvent::Settings(event) => this.on_settings(event, cx),
                    ControlEvent::Preview(index) => {
                        if let Some(item) = this.doc.items.get(index) {
                            this.selected = Some(item.id);
                            cx.notify();
                        }
                    }
                });
                if handled.is_err() {
                    break;
                }
            }
        })
        .detach();
        Some(controls)
    }

    /// Brings the AppKit views up to date with the box: the table rows and selection, and the
    /// item count under the window title.
    #[cfg(target_os = "macos")]
    fn sync_controls(&mut self) {
        let status = if self.drop_preview.is_some() || ambry_macos::settings::shows_item_count() {
            self.status().to_string()
        } else {
            String::new()
        };
        let area = self.scroll_area();
        let (x, y) = (-f32::from(self.pan.x), -f32::from(self.pan.y));
        let (width, height) = (
            f32::from(self.viewport.width),
            f32::from(self.viewport.height),
        );
        // The scroll view sits under the toolbar, so its document is the bar shorter than
        // the canvas.
        let scroll = [
            f64::from((area.2 - area.0).max(width)),
            f64::from((area.3 - area.1).max(height) - layout::BAR as f32),
            f64::from(x - area.0),
            f64::from(y - area.1),
        ];
        let in_canvas = self.mode == Mode::Canvas;
        let (zoom, dark) = (self.zoom, DARK_MODE.get());
        let Some(controls) = &mut self.controls else {
            return;
        };
        controls.toolbar_blur.set_color(palette().canvas);
        let items = self.doc.items.len();
        let shown = (box_label(&self.doc.name, &self.doc.icon, items), status);
        if controls.shown_box.as_ref() != Some(&shown) {
            controls.toolbar.set_box(&shown.0, &shown.1);
            controls.shown_box = Some(shown);
        }
        // The open box shows what it holds now, not what was last saved.
        let current = self
            .boxes
            .iter()
            .position(|entry| entry.path == self.store.path());
        let boxes: Vec<ambry_macos::BoxLabel> = self
            .boxes
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let items = if Some(index) == current {
                    items
                } else {
                    entry.items
                };
                box_label(&entry.name, &entry.icon, items)
            })
            .collect();
        if controls.shown_boxes != (boxes.clone(), current) {
            controls.toolbar.set_boxes(boxes.clone(), current);
            controls.shown_boxes = (boxes, current);
        }
        if controls.shown_scroll != Some(scroll) {
            let [width, height, x, y] = scroll;
            controls.scroll.set(width, height, x, y);
            controls.shown_scroll = Some(scroll);
        }
        controls.scroll.native().set_hidden(!in_canvas);
        controls
            .zoom_label
            .set_percent((zoom * 100.).round() as i32, dark);
        controls.zoom_label.native().set_hidden(!in_canvas);
        controls.columns.native().set_hidden(in_canvas);
        controls.toolbar.set_bar(!in_canvas);
        let paths: Vec<PathBuf> = self
            .doc
            .items
            .iter()
            .map(|item| item.path.clone())
            .collect();
        controls.columns.set_items(&paths);
        // Only the canvas passes under the toolbar; the list starts below it.
        controls.toolbar_blur.set_hidden(!in_canvas);
    }

    /// Renders Finder's icon for every item that has none yet, or whose file changed since:
    /// a folder's shows the documents peeking out of it once it has some.
    fn load_icons(&mut self) {
        let paths: Vec<PathBuf> = self
            .doc
            .items
            .iter()
            .chain(self.drop_preview.iter().flat_map(|preview| &preview.items))
            .filter(|item| !item.is_image())
            .map(|item| item.path.clone())
            .collect();
        for path in paths {
            let stamp = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok();
            if self.icons.get(&path).is_some_and(|(seen, _)| *seen == stamp) {
                continue;
            }
            let icon = finder_icon_file(&path, stamp);
            self.icons.insert(path, (stamp, icon));
        }
        #[cfg(target_os = "macos")]
        for item in &self.doc.items {
            self.details
                .entry(item.path.clone())
                .or_insert_with_key(|path| file_details(path));
        }
    }

    #[cfg(target_os = "macos")]
    fn on_settings(&mut self, event: ambry_macos::settings::SettingsEvent, cx: &mut Context<Self>) {
        use ambry_macos::settings::SettingsEvent;
        match event {
            SettingsEvent::Account => {
                self.avatar = avatar_path().filter(|path| path.exists());
                if let Some(controls) = &self.controls {
                    controls
                        .toolbar
                        .set_account(avatar_path().as_deref(), &user_initial());
                }
            }
            SettingsEvent::ItemCount => {}
        }
        cx.notify();
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        #[cfg(target_os = "macos")]
        if let Some(controls) = &self.controls {
            controls
                .toolbar
                .select_mode(if mode == Mode::Canvas { 0 } else { 1 });
        }
        cx.notify();
    }

    fn save(&mut self) {
        if let Err(err) = self.store.save(&self.doc) {
            eprintln!("could not save {}: {err:#}", self.store.path().display());
        }
        self.boxes = ambry_core::list_boxes(self.store.dir());
    }

    /// Renames box `index` of [`Self::boxes`], this one or another saved one.
    #[cfg(target_os = "macos")]
    fn rename_box(&mut self, index: usize, name: String, cx: &mut Context<Self>) {
        let Some(entry) = self.boxes.get(index).cloned() else {
            return;
        };
        if entry.path == self.store.path() {
            self.doc.name = name;
            self.save();
        } else {
            let store = Store::new(entry.path);
            match store.load_or_new(&entry.name) {
                Ok(mut doc) => {
                    doc.name = name;
                    if let Err(err) = store.save(&doc) {
                        eprintln!("could not save {}: {err:#}", store.path().display());
                    }
                }
                Err(err) => eprintln!("could not load {}: {err:#}", store.path().display()),
            }
            self.boxes = ambry_core::list_boxes(self.store.dir());
        }
        cx.notify();
    }

    /// Saves this box and shows the one `store` holds instead.
    #[cfg(target_os = "macos")]
    fn open_box(&mut self, store: Store, cx: &mut Context<Self>) {
        if store.path() == self.store.path() {
            return;
        }
        self.save();
        let doc = store.load_or_new("Untitled").unwrap_or_else(|err| {
            eprintln!("could not load {}: {err:#}", store.path().display());
            BoxDoc::new("Untitled")
        });
        self.appear_delay = doc
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| (item.id, index.min(12) as f32 * 0.035))
            .collect();
        self.doc = doc;
        self.store = store;
        self.selected = None;
        self.picked.clear();
        self.drag = None;
        self.pan = point(px(0.), px(0.));
        self.zoom = 1.;
        self.boxes = ambry_core::list_boxes(self.store.dir());
        self.set_mode(default_mode(), cx);
    }

    /// "New Box", or "New Box 2" and up when that name is taken.
    #[cfg(target_os = "macos")]
    fn new_box_name(&self) -> String {
        let taken = |name: &str| self.boxes.iter().any(|entry| entry.name == name);
        let mut name = "New Box".to_string();
        let mut n = 2;
        while taken(&name) {
            name = format!("New Box {n}");
            n += 1;
        }
        name
    }

    /// The part of the canvas one can scroll over, as `(left, top, right, bottom)` in canvas
    /// coordinates: the items with room around them, and always the first screen.
    fn scroll_area(&self) -> (f32, f32, f32, f32) {
        let mut area: (f32, f32, f32, f32) = (
            0.,
            0.,
            f32::from(self.viewport.width),
            f32::from(self.viewport.height),
        );
        // Zooming spreads the items apart; the items themselves keep their size.
        for item in &self.doc.items {
            let (x, y) = (item.x * self.zoom, item.y * self.zoom);
            area.0 = area.0.min(x - CANVAS_MARGIN);
            area.1 = area.1.min(y - CANVAS_MARGIN);
            area.2 = area.2.max(x + ITEM_WIDTH + CANVAS_MARGIN);
            area.3 = area.3.max(y + ITEM_HEIGHT + CANVAS_MARGIN);
        }
        area
    }

    /// Where the view comes to rest: `pan` held inside [`Self::scroll_area`].
    fn rest_pan(&self) -> Point<Pixels> {
        let area = self.scroll_area();
        let clamp = |pan: Pixels, min: f32, max: f32, visible: Pixels| {
            let at = -f32::from(pan);
            let at = at.min(max - f32::from(visible)).max(min);
            px(-at)
        };
        point(
            clamp(self.pan.x, area.0, area.2, self.viewport.width),
            clamp(self.pan.y, area.1, area.3, self.viewport.height),
        )
    }

    /// Keeps the view inside [`Self::scroll_area`].
    fn clamp_pan(&mut self) {
        self.pan = self.rest_pan();
    }

    /// Moves the canvas by `delta` and shows the scroll bars while it moves.
    fn scroll_by(&mut self, delta: Point<Pixels>, cx: &mut Context<Self>) {
        self.pan = self.pan + delta;
        self.clamp_pan();
        #[cfg(target_os = "macos")]
        if let Some(controls) = &self.controls {
            controls.scroll.flash();
        }
        cx.notify();
    }

    /// Moves the canvas under a trackpad the way an NSScrollView does: past an edge the canvas
    /// follows the fingers with growing resistance, and the moment they lift, or the coasting
    /// after a flick runs into the edge, it springs back.
    fn scroll_elastic(
        &mut self,
        delta: Point<Pixels>,
        phase: gpui::TouchPhase,
        cx: &mut Context<Self>,
    ) {
        match phase {
            gpui::TouchPhase::Started => self.fingers = true,
            gpui::TouchPhase::Ended => self.fingers = false,
            gpui::TouchPhase::Moved => {}
        }
        let rest = self.rest_pan();
        let pull = |pan: Pixels, rest: Pixels, delta: Pixels, extent: Pixels| {
            // The fingers' own travel past the edge, from which AppKit's rubber band curve
            // gives how far the canvas goes: it never quite reaches the size of the view.
            let over = f32::from(pan - rest);
            let extent = f32::from(extent).max(1.);
            let travel = unrubber(over, extent) + f32::from(delta);
            if travel * over > 0. {
                px(f32::from(rest) + rubber(travel, extent))
            } else {
                // Back over the edge: the rest of the move is inside, one to one.
                px(f32::from(rest) + travel)
            }
        };
        let moved = point(self.pan.x + delta.x, self.pan.y + delta.y);
        let rest_moved = {
            let saved = self.pan;
            self.pan = moved;
            let rest = self.rest_pan();
            self.pan = saved;
            rest
        };
        // Inside the edges the canvas moves one to one; only what goes past one stretches.
        let axis = |pan: Pixels,
                    moved: Pixels,
                    rest: Pixels,
                    rest_moved: Pixels,
                    delta: Pixels,
                    extent: Pixels| {
            if pan == rest && moved == rest_moved {
                moved
            } else if pan == rest {
                // Reaching the edge in this step: up to it freely, the rest stretched.
                let free = rest_moved - pan;
                let left = f32::from(delta) - f32::from(free);
                px(f32::from(rest_moved) + rubber(left, f32::from(extent).max(1.)))
            } else {
                pull(pan, rest, delta, extent)
            }
        };
        self.pan = point(
            axis(
                self.pan.x,
                moved.x,
                rest.x,
                rest_moved.x,
                delta.x,
                self.viewport.width,
            ),
            axis(
                self.pan.y,
                moved.y,
                rest.y,
                rest_moved.y,
                delta.y,
                self.viewport.height,
            ),
        );
        let stretched = self.pan != self.rest_pan();
        if stretched && !self.fingers && !self.springing {
            self.spring_back(cx);
        }
        #[cfg(target_os = "macos")]
        if let Some(controls) = &self.controls {
            controls.scroll.flash();
        }
        cx.notify();
    }

    /// Springs a canvas pulled past an edge back to it, on a critically damped spring like
    /// AppKit's.
    fn spring_back(&mut self, cx: &mut Context<Self>) {
        self.springing = true;
        self.pan_velocity = point(px(0.), px(0.));
        cx.spawn(async move |this, cx| {
            loop {
                let more = this
                    .update(cx, |this, cx| {
                        if this.fingers {
                            this.springing = false;
                            return false;
                        }
                        let rest = this.rest_pan();
                        let dt = 1. / 60.;
                        let omega = 2. * std::f32::consts::PI / SPRING_RESPONSE;
                        let step = |pan: Pixels, velocity: Pixels, rest: Pixels| {
                            let offset = f32::from(pan - rest);
                            let velocity = f32::from(velocity);
                            let accel = -omega * omega * offset - 2. * omega * velocity;
                            let velocity = velocity + accel * dt;
                            (px(f32::from(rest) + offset + velocity * dt), px(velocity))
                        };
                        let (x, vx) = step(this.pan.x, this.pan_velocity.x, rest.x);
                        let (y, vy) = step(this.pan.y, this.pan_velocity.y, rest.y);
                        this.pan = point(x, y);
                        this.pan_velocity = point(vx, vy);
                        let near = |a: Pixels, b: Pixels, v: Pixels| {
                            f32::from(a - b).abs() < 0.3 && f32::from(v).abs() < 5.
                        };
                        let done = near(x, rest.x, vx) && near(y, rest.y, vy);
                        if done {
                            this.pan = rest;
                            this.springing = false;
                        }
                        cx.notify();
                        !done
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
            }
        })
        .detach();
    }

    /// Follows a pinch, `step` more magnification about `at`, as Maps and Photos do: past the
    /// smallest or largest zoom it keeps going with growing resistance, and when the fingers
    /// lift (`step` 0) it springs back to the limit.
    fn pinch(&mut self, step: f32, at: Point<Pixels>, cx: &mut Context<Self>) {
        if self.mode != Mode::Canvas {
            return;
        }
        if step == 0. {
            self.pinch_raw = None;
            return self.spring_zoom(at, cx);
        }
        let raw = self.pinch_raw.unwrap_or(self.zoom) * (1. + step);
        self.pinch_raw = Some(raw);
        let shown = if raw > MAX_ZOOM {
            MAX_ZOOM * (raw / MAX_ZOOM).powf(RUBBER)
        } else if raw < MIN_ZOOM {
            MIN_ZOOM * (raw / MIN_ZOOM).powf(RUBBER)
        } else {
            raw
        };
        self.zoom_to(shown, at, cx);
    }

    /// Eases the zoom back inside its limits after a pinch went past them.
    fn spring_zoom(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.scroll_gen += 1;
        let generation = self.scroll_gen;
        cx.spawn(async move |this, cx| {
            loop {
                let more = this
                    .update(cx, |this, cx| {
                        if this.scroll_gen != generation || this.pinch_raw.is_some() {
                            return false;
                        }
                        let rest = this.zoom.clamp(MIN_ZOOM, MAX_ZOOM);
                        let gap = rest - this.zoom;
                        let done = gap.abs() < 0.002;
                        let next = if done { rest } else { this.zoom + gap * 0.25 };
                        this.zoom_to(next, at, cx);
                        !done
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
            }
        })
        .detach();
    }

    /// Zooms by `factor` about the window position `at`, which stays under the pointer.
    fn zoom_at(&mut self, factor: f32, at: Point<Pixels>, cx: &mut Context<Self>) {
        if self.mode != Mode::Canvas {
            return;
        }
        let new = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.zoom_to(new, at, cx);
    }

    /// Shows the canvas at zoom `new` about the window position `at`.
    fn zoom_to(&mut self, new: f32, at: Point<Pixels>, cx: &mut Context<Self>) {
        let old = self.zoom;
        if (new - old).abs() < f32::EPSILON {
            return;
        }
        let ratio = new / old;
        self.pan = point(
            at.x - (at.x - self.pan.x) * ratio,
            at.y - (at.y - self.pan.y) * ratio,
        );
        self.zoom = new;
        self.clamp_pan();
        #[cfg(target_os = "macos")]
        if let Some(controls) = &self.controls {
            controls.scroll.flash();
        }
        cx.notify();
    }

    /// The middle of the visible canvas, which keys and the zoom label zoom about.
    fn viewport_centre(&self) -> Point<Pixels> {
        point(self.viewport.width / 2., self.viewport.height / 2.)
    }

    /// Follows a drag of a scroll bar's knob or a click in its track, which moved the visible
    /// part of the canvas to `(x, y)` from the top-left of [`Self::scroll_area`].
    #[cfg(target_os = "macos")]
    fn on_scroller(&mut self, x: f64, y: f64, cx: &mut Context<Self>) {
        let area = self.scroll_area();
        self.pan = point(px(-(x as f32 + area.0)), px(-(y as f32 + area.1)));
        self.clamp_pan();
        cx.notify();
    }

    /// Converts a window position into canvas coordinates.
    fn to_canvas(&self, position: Point<Pixels>) -> (f32, f32) {
        (
            f32::from(position.x - self.pan.x) / self.zoom,
            f32::from(position.y - self.pan.y) / self.zoom,
        )
    }

    /// Top-left corner of an item in window coordinates. The item is pinned to the canvas by
    /// the middle of its top edge, where its icon hangs from, so zooming keeps every icon on
    /// its own spot instead of sliding it against the dots.
    fn item_origin(&self, item: &Item) -> Point<Pixels> {
        point(
            self.screen_left(item.x),
            self.pan.y + px(item.y * self.zoom),
        )
    }

    /// The window x of the left edge of an item whose canvas left is `x`.
    fn screen_left(&self, x: f32) -> Pixels {
        self.pan.x + px((x + ITEM_WIDTH / 2.) * self.zoom - ITEM_WIDTH / 2.)
    }

    /// The canvas left of an item drawn with its left edge at window x `left`.
    fn canvas_left(&self, left: Pixels) -> f32 {
        (f32::from(left - self.pan.x) + ITEM_WIDTH / 2.) / self.zoom - ITEM_WIDTH / 2.
    }

    /// Where the first of `count` dropped items lands: centred under the cursor,
    /// or in the middle of the visible canvas when there is no cursor position.
    fn drop_origin(&self, count: usize, at: Option<Point<Pixels>>) -> (f32, f32) {
        let row_width = count.saturating_sub(1) as f32 * DROP_STEP_X;
        let position = match (self.mode, at) {
            (Mode::Canvas, Some(position)) => position,
            _ => point(self.viewport.width / 2., self.viewport.height / 2.),
        };
        let (x, y) = self.to_canvas(position);
        // Items hang from the middle of their top edge, so the row is centred on the cursor
        // in canvas units; the card's height shrinks and grows with the zoom.
        (x - ITEM_WIDTH / 2. - row_width / 2., y - CARD_HEIGHT / 2.)
    }

    fn add_paths(
        &mut self,
        paths: Vec<PathBuf>,
        at: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let (x, y) = self.drop_origin(paths.len(), at);
        let added = self.doc.add_paths(&paths, x, y);
        for (index, id) in added.iter().enumerate() {
            self.appear_delay.insert(*id, index as f32 * 0.05);
            self.landed
                .insert(*id, (std::time::Instant::now(), Landing::Added));
        }
        if let Some(&last) = added.last() {
            self.selected = Some(last);
            self.save();
        } else if let Some(existing) = paths
            .first()
            .and_then(|path| self.doc.items.iter().find(|item| &item.path == path))
        {
            // Already in the box: point at it instead of adding a duplicate.
            self.selected = Some(existing.id);
        }
        self.drop_preview = None;
        cx.notify();
    }

    fn preview_drop(&mut self, position: Point<Pixels>, paths: &[PathBuf], cx: &mut Context<Self>) {
        let same_paths = self.drop_preview.as_ref().is_some_and(|preview| {
            preview.items.len() == paths.len()
                && preview
                    .items
                    .iter()
                    .zip(paths)
                    .all(|(item, path)| &item.path == path)
        });
        if !same_paths {
            let mut ghosts = BoxDoc::new("");
            ghosts.add_paths(paths, 0., 0.);
            self.drop_preview = Some(DropPreview {
                position,
                items: ghosts.items,
            });
        } else if let Some(preview) = &mut self.drop_preview {
            preview.position = position;
        }
        cx.notify();
    }

    fn pick_paths(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Add to box".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |this, cx| this.add_paths(paths, None, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn open_item(&self, id: u64, cx: &mut App) {
        if let Some(item) = self.doc.item(id) {
            cx.open_with_system(&item.path);
        }
    }

    fn on_item_mouse_down(
        &mut self,
        id: u64,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.focus(&self.focus);
        self.selected = Some(id);
        self.picked.clear();
        #[cfg(target_os = "macos")]
        self.preview(false);
        if event.click_count >= 2 {
            self.drag = None;
            self.open_item(id, cx);
        } else if self.mode == Mode::Canvas {
            if let Some(item) = self.doc.item(id) {
                self.drag = Some(Drag {
                    id,
                    grab: event.position - self.item_origin(item),
                    moved: false,
                    at: event.position,
                });
            }
        }
        cx.notify();
    }

    /// How much bigger than its size item `id` is drawn now: lifted while dragged, and
    /// bouncing for a moment after it lands.
    fn pop(&self, id: u64, lifted: bool) -> f32 {
        if lifted {
            let t = self.lifted_at.map_or(1., |at| at.elapsed().as_secs_f32());
            return 1. + LIFT * (1. - (-t / 0.06).exp());
        }
        let Some((at, landing)) = self.landed.get(&id) else {
            return 1.;
        };
        let delay = self.appear_delay.get(&id).copied().unwrap_or(0.);
        let t = at.elapsed().as_secs_f32();
        match landing {
            Landing::Added => {
                let t = (t - delay).max(0.);
                1. - 0.45 * (-7. * t).exp() * (13. * t).cos()
            }
            Landing::Dropped => 1. + LIFT * (-8. * t).exp() * (16. * t).cos(),
        }
    }

    /// Whether some item is still bouncing or growing, so frames keep coming.
    fn popping(&self) -> bool {
        self.lifted_at
            .is_some_and(|at| at.elapsed().as_secs_f32() < 0.4)
            || self
                .landed
                .values()
                .any(|(at, _)| at.elapsed().as_secs_f32() < 1.2)
    }

    /// The folder on the canvas under what is being dragged: an item of the box, or files
    /// coming in from Finder.
    fn folder_under_drag(&self) -> Option<u64> {
        let (at, dragged) = match (&self.drag, &self.drop_preview) {
            (Some(drag), _) if drag.moved => (drag.at, Some(drag.id)),
            (_, Some(preview)) => (preview.position, None),
            _ => return None,
        };
        let side = ICON_SIZE * self.zoom + 8.;
        self.doc
            .items
            .iter()
            .rev()
            .filter(|item| item.kind == ItemKind::Folder && Some(item.id) != dragged)
            .find(|item| {
                let middle = self.screen_left(item.x) + px(ITEM_WIDTH / 2.);
                let top = self.pan.y + px(item.y * self.zoom);
                (at.x - middle).abs() <= px(side / 2.) && at.y >= top && at.y <= top + px(side)
            })
            .map(|item| item.id)
    }

    fn is_picked(&self, id: u64) -> bool {
        self.selected == Some(id) || self.picked.contains(&id)
    }

    fn start_marquee(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.selected = None;
        self.picked.clear();
        self.marquee = Some(Marquee { start: at, now: at });
        cx.notify();
    }

    /// Picks the items under the rectangle being dragged.
    fn update_marquee(&mut self) {
        let Some(marquee) = self.marquee else { return };
        let (left, right) = (
            f32::from(marquee.start.x.min(marquee.now.x)),
            f32::from(marquee.start.x.max(marquee.now.x)),
        );
        let (top, bottom) = (
            f32::from(marquee.start.y.min(marquee.now.y)),
            f32::from(marquee.start.y.max(marquee.now.y)),
        );
        let hit =
            |x0: f32, y0: f32, x1: f32, y1: f32| left < x1 && right > x0 && top < y1 && bottom > y0;
        let picked: Vec<u64> = match self.mode {
            Mode::Canvas => self
                .doc
                .items
                .iter()
                .filter(|item| {
                    let origin = self.item_origin(item);
                    let middle = f32::from(origin.x) + ITEM_WIDTH / 2.;
                    let top = f32::from(origin.y);
                    hit(middle - 40., top, middle + 40., top + ITEM_HEIGHT)
                })
                .map(|item| item.id)
                .collect(),
            // The column view picks by dragging on its own.
            Mode::Columns => Vec::new(),
        };
        self.selected = picked.last().copied();
        self.picked = picked.into_iter().collect();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.marquee.is_some() {
            if event.pressed_button != Some(MouseButton::Left) {
                self.marquee = None;
            } else {
                if let Some(marquee) = &mut self.marquee {
                    marquee.now = event.position;
                }
                self.update_marquee();
            }
            cx.notify();
            return;
        }
        let Some(drag) = &self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_drag(cx);
            return;
        }
        let id = drag.id;
        let origin = event.position - drag.grab;
        if let Some(drag) = &mut self.drag {
            drag.at = event.position;
        }
        let (_, y) = self.to_canvas(origin);
        let x = self.canvas_left(origin.x);
        self.doc.move_item(id, x, y);
        if let Some(drag) = &mut self.drag {
            if !drag.moved {
                self.lifted_at = Some(std::time::Instant::now());
            }
            drag.moved = true;
        }
        cx.notify();
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.marquee.take().is_some() {
            cx.notify();
        }
        if let Some(drag) = self.drag.take() {
            if drag.moved {
                self.landed
                    .insert(drag.id, (std::time::Instant::now(), Landing::Dropped));
                self.save();
            }
            self.lifted_at = None;
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Command with plus, minus or zero zooms like every Mac app.
        if event.keystroke.modifiers.platform {
            let centre = self.viewport_centre();
            match event.keystroke.key.as_str() {
                "=" | "+" => return self.zoom_at(ZOOM_STEP, centre, cx),
                "-" => return self.zoom_at(1. / ZOOM_STEP, centre, cx),
                "0" => return self.zoom_at(1. / self.zoom, centre, cx),
                "1" => return self.set_mode(Mode::Canvas, cx),
                "2" => return self.set_mode(Mode::Columns, cx),
                _ => {}
            }
        }
        match event.keystroke.key.as_str() {
            "backspace" | "delete" => {
                let mut ids: Vec<u64> = self.picked.drain().collect();
                ids.extend(self.selected.take());
                if !ids.is_empty() {
                    for id in ids {
                        self.doc.remove(id);
                    }
                    self.save();
                    cx.notify();
                }
            }
            "escape" => {
                self.selected = None;
                self.picked.clear();
                cx.notify();
            }
            "enter" => {
                if let Some(id) = self.selected {
                    self.open_item(id, cx);
                }
            }
            #[cfg(target_os = "macos")]
            "space" => self.preview(true),
            _ => {}
        }
    }

    /// Opens or closes Quick Look on the selected item with `toggle`, or else shows the
    /// selected item in it when it is open, as Finder does with the space bar.
    #[cfg(target_os = "macos")]
    fn preview(&self, toggle: bool) {
        let Some(index) = self
            .selected
            .and_then(|id| self.doc.items.iter().position(|item| item.id == id))
        else {
            return;
        };
        let paths: Vec<PathBuf> = self
            .doc
            .items
            .iter()
            .map(|item| item.path.clone())
            .collect();
        if !toggle {
            ambry_macos::quick_look::follow(&paths, index);
            return;
        }
        let Some(events) = self
            .controls
            .as_ref()
            .map(|controls| controls.events.clone())
        else {
            return;
        };
        ambry_macos::quick_look::toggle(&paths, index, move |index| {
            events.try_send(ControlEvent::Preview(index)).ok();
        });
    }

    fn status(&self) -> SharedString {
        if let Some(preview) = &self.drop_preview {
            return match preview.items.len() {
                1 => "Adding 1 item".into(),
                n => format!("Adding {n} items").into(),
            };
        }
        match self.doc.items.len() {
            0 => "Empty".into(),
            1 => "1 item".into(),
            n => format!("{n} items").into(),
        }
    }

    fn render_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (pan, zoom, between) = (self.pan, self.zoom, self.between_dots);
        let mut items: Vec<gpui::AnyElement> = self
            .doc
            .items
            .iter()
            .map(|item| self.render_canvas_item(item, cx).into_any_element())
            .collect();
        if let Some(preview) = &self.drop_preview {
            items.extend(self.render_ghosts(preview));
        }

        // Under the toolbar too, as Finder's content runs under its glass: the toolbar stays
        // clear over the canvas and the soft shade keeps it readable.
        div()
            .id("canvas")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    this.start_marquee(event.position, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height());
                // Scrolling with Command or Control held zooms, as it does in Maps and Finder.
                if event.modifiers.platform || event.modifiers.control {
                    let factor = (1. + f32::from(delta.y) * 0.01).max(0.5);
                    this.zoom_at(factor, event.position, cx);
                } else if matches!(event.delta, gpui::ScrollDelta::Pixels(_)) {
                    this.scroll_elastic(delta, event.touch_phase, cx);
                } else {
                    this.scroll_by(delta, cx);
                }
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| paint_dots(bounds, pan, zoom, between, window),
                )
                .absolute()
                .size_full(),
            )
            .when(
                self.doc.items.is_empty() && self.drop_preview.is_none(),
                |this| this.child(empty_hint()),
            )
            .children(items)
            .children(edge_fades())
    }

    fn render_canvas_item(&self, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let id = item.id;
        let selected = self.is_picked(id);
        let lifted = self
            .drag
            .as_ref()
            .is_some_and(|drag| drag.id == id && drag.moved);
        let left = self.screen_left(item.x);
        let top = self.pan.y + px(item.y * self.zoom) - if lifted { px(3.) } else { px(0.) };
        let delay = self.appear_delay.get(&id).copied().unwrap_or(0.);
        let pop = self.pop(id, lifted);
        let fade = selected || self.was_selected.borrow().contains(&id);
        if selected {
            self.was_selected.borrow_mut().insert(id);
        }
        let open = self.open_folder == Some(id);
        let closing = !open && self.was_open.borrow().contains(&id);
        if open {
            self.was_open.borrow_mut().insert(id);
        }
        let face = match (&self.open_icon, self.icon(item)) {
            (Some(open_icon), Some(icon))
                if item.kind == ItemKind::Folder && (open || closing) =>
            {
                folder_face(icon, open_icon.clone(), open, id, self.zoom * pop)
            }
            (_, icon) => finder_face(item, icon, false, lifted, self.zoom * pop),
        };
        let face = if fade {
            face.with_animation(
                (if selected { "select" } else { "deselect" }, id as usize),
                Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
                move |this, t| {
                    let mut tint = palette().selection;
                    tint.a *= if selected { t } else { 1. - t };
                    this.bg(tint)
                },
            )
            .into_any_element()
        } else {
            face.into_any_element()
        };
        let label = item_label(item.name(), false, item.exists());
        let label = if fade {
            label
                .with_animation(
                    (
                        if selected {
                            "select-label"
                        } else {
                            "deselect-label"
                        },
                        id as usize,
                    ),
                    Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
                    move |this, t| {
                        let on = if selected { t } else { 1. - t };
                        let mut accent: Hsla = rgb(ACCENT).into();
                        accent.a = on;
                        this.bg(accent)
                            .when(on > 0.5, |this| this.text_color(gpui::white()))
                    },
                )
                .into_any_element()
        } else {
            label.into_any_element()
        };

        div()
            .id(("item", id as usize))
            .absolute()
            .left(left)
            .top(top)
            .w(px(ITEM_WIDTH))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .cursor(if lifted {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::Arrow
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, window, cx| {
                    this.on_item_mouse_down(id, event, window, cx)
                }),
            )
            .child(face)
            .child(label)
            .with_animation(
                ("appear", id as usize),
                Animation::new(Duration::from_secs_f32(0.45 + delay))
                    .with_easing(delayed(delay / (0.45 + delay), ease_out_quint())),
                move |this, t| this.top(top - px(10. * (1. - t))).opacity(t),
            )
    }

    fn render_ghosts(&self, preview: &DropPreview) -> Vec<gpui::AnyElement> {
        let (x, y) = self.drop_origin(preview.items.len(), Some(preview.position));
        preview
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let top = self.pan.y + px(y * self.zoom);
                div()
                    .absolute()
                    .left(self.screen_left(x + index as f32 * DROP_STEP_X))
                    .top(top)
                    .w(px(ITEM_WIDTH))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .child(ghost_face(item, self.icon(item), self.zoom))
                    .child(item_label(item.name(), false, true))
                    .with_animation(
                        ("ghost", index),
                        Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()),
                        move |this, t| this.top(top + px(8. * (1. - t))).opacity(0.75 * t),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn icon(&self, item: &Item) -> Option<PathBuf> {
        self.icons.get(&item.path).and_then(|(_, icon)| icon.clone())
    }

    /// Finder's column view, AppKit's own browser. It runs under the toolbar as Finder's
    /// does: its columns inset themselves below the bar and scroll under its glass.
    #[cfg(target_os = "macos")]
    fn render_columns(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .when_some(self.controls.as_ref(), |this, controls| {
                this.child(native(controls.columns.native()).fill())
            })
    }

    #[cfg(not(target_os = "macos"))]
    fn render_columns(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("list")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .overflow_y_scroll()
            .px(px(24.))
            .pt(px(8.))
            .pb(px(96.))
            .flex()
            .flex_col()
            .text_size(px(LABEL_SIZE))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    this.selected = None;
                    cx.notify();
                }),
            )
            .when(self.doc.items.is_empty(), |this| this.child(empty_hint()))
            .children(self.doc.items.iter().enumerate().map(|(index, item)| {
                let id = item.id;
                let selected = self.selected == Some(id);
                div()
                    .id(("row", id as usize))
                    .h(px(36.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(10.))
                    .rounded(px(8.))
                    .when(selected, |this| this.bg(rgb(palette().pressed)))
                    .when(!item.exists(), |this| this.opacity(0.4))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.on_item_mouse_down(id, event, window, cx)
                        }),
                    )
                    .child(
                        div()
                            .w(px(52.))
                            .text_color(rgb(palette().faint))
                            .child(item.badge()),
                    )
                    .child(div().text_color(rgb(palette().ink)).child(item.name()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(rgb(palette().faint))
                            .truncate()
                            .child(item.path.display().to_string()),
                    )
                    .with_animation(
                        ("row-appear", id as usize),
                        Animation::new(Duration::from_millis(260 + 20 * index.min(12) as u64))
                            .with_easing(ease_out_quint()),
                        |this, t| this.opacity(t),
                    )
            }))
    }

    #[cfg(not(target_os = "macos"))]
    fn render_avatar(&self) -> impl IntoElement {
        let size = px(layout::AVATAR as f32);
        div()
            .id("account")
            .size(size)
            .rounded_full()
            .bg(rgb(palette().canvas))
            .occlude()
            .shadow(vec![
                shadow(0., 4., 16., 0.06),
                shadow(0., 1., 4., 0.17),
                shadow(0., 0., 1., 0.3),
            ])
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(TITLE_SIZE))
            .text_color(rgb(palette().ink))
            .map(|this| match &self.avatar {
                Some(path) => this.child(
                    img(path.clone())
                        .size(size)
                        .rounded_full()
                        .object_fit(ObjectFit::Cover),
                ),
                None => this.child(user_initial()),
            })
    }

    /// The add button, the canvas/list switch and the account avatar, top right, drawn by
    /// GPUI where AppKit and its toolbar are not available.
    #[cfg(not(target_os = "macos"))]
    fn render_top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .top(px(layout::TOP as f32))
            .right(px(layout::TOP as f32))
            .flex()
            .items_center()
            .gap(px(layout::GAP_BEFORE_AVATAR as f32))
            .child(self.render_fallback_toolbar(cx))
            .child(self.render_avatar())
    }

    /// The canvas/list switch and add button drawn by GPUI where AppKit is not available.
    #[cfg(not(target_os = "macos"))]
    fn render_fallback_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let height = px(layout::HEIGHT as f32);
        let mode = self.mode;
        let mode_button = move |id: &'static str, icon: &'static str, target: Mode| {
            let active = mode == target;
            div()
                .id(id)
                .size(px(28.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .when(active, |this| this.bg(rgb(palette().pressed)))
                .child(svg().path(icon).size(px(18.)).text_color(if active {
                    rgb(palette().ink)
                } else {
                    rgb(palette().quiet)
                }))
        };
        div()
            .flex()
            .gap(px(layout::GAP as f32))
            .child(
                floating()
                    .id("mode")
                    .w(px(layout::SEGMENTS_WIDTH as f32))
                    .h(height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(2.))
                    .child(
                        mode_button("mode-canvas", "icons/frame.svg", Mode::Canvas).on_click(
                            cx.listener(|this, _, _, cx| this.set_mode(Mode::Canvas, cx)),
                        ),
                    )
                    .child(
                        mode_button("mode-list", "icons/list.svg", Mode::Columns).on_click(
                            cx.listener(|this, _, _, cx| this.set_mode(Mode::Columns, cx)),
                        ),
                    ),
            )
            .child(
                floating()
                    .id("add")
                    .size(height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_click(cx.listener(|this, _, _, cx| this.pick_paths(cx)))
                    .child(
                        svg()
                            .path("icons/plus.svg")
                            .size(px(18.))
                            .text_color(rgb(palette().ink)),
                    ),
            )
    }
}

impl Render for BoxView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The in-between dots switch at the middle of each halving, with a margin either side
        // so a zoom resting near it does not flicker.
        let mut spacing = DOT_SPACING * self.zoom;
        while spacing < DOT_SPACING / 2. {
            spacing *= 2.;
        }
        let closeness = (spacing - DOT_SPACING / 2.) / (DOT_SPACING / 2.);
        let (dense, since) = self.dense_dots;
        if (dense && closeness < 0.4) || (!dense && closeness > 0.6) {
            self.dense_dots = (!dense, std::time::Instant::now());
        }
        let (dense, since) = (self.dense_dots.0, self.dense_dots.1.max(since));
        let t = (since.elapsed().as_secs_f32() / DOT_FADE).min(1.);
        let t = 1. - (1. - t).powi(3);
        self.between_dots = if dense { t } else { 1. - t };
        if t < 1. {
            window.request_animation_frame();
        }
        if self.popping() {
            window.request_animation_frame();
        } else {
            self.landed.clear();
        }
        self.viewport = window.viewport_size();
        self.open_folder = self.folder_under_drag();
        // A canvas pulled past an edge stays there until it springs back.
        if self.drag.is_none() && !self.fingers && !self.springing {
            self.clamp_pan();
        }
        DARK_MODE.set(matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ));
        if !cx.has_active_drag() {
            self.drop_preview = None;
        }
        self.load_icons();
        #[cfg(target_os = "macos")]
        self.sync_controls();
        #[cfg(target_os = "macos")]
        window.set_window_title(&self.doc.name);
        #[cfg(not(target_os = "macos"))]
        window.set_window_title(&format!("{} — {}", self.doc.name, self.status()));

        let root = div()
            .id("box")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(rgb(palette().canvas))
            .text_color(rgb(palette().ink))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.end_drag(cx)),
            )
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                    let paths = event.drag(cx).paths().to_vec();
                    this.preview_drop(event.event.position, &paths, cx);
                }),
            )
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.add_paths(paths.paths().to_vec(), Some(window.mouse_position()), cx);
            }))
            .map(|this| match self.mode {
                Mode::Canvas => this.child(self.render_canvas(cx)),
                Mode::Columns => this.child(self.render_columns(cx)),
            })
            .when_some(self.marquee, |this, marquee| {
                let (left, top) = (
                    marquee.start.x.min(marquee.now.x),
                    marquee.start.y.min(marquee.now.y),
                );
                this.child(
                    div()
                        .absolute()
                        .left(left)
                        .top(top)
                        .w(marquee.start.x.max(marquee.now.x) - left)
                        .h(marquee.start.y.max(marquee.now.y) - top)
                        .rounded(px(2.))
                        .border_1()
                        .border_color(hsla(216. / 360., 0.92, 0.57, 0.8))
                        .bg(hsla(216. / 360., 0.92, 0.57, 0.12)),
                )
            });
        #[cfg(not(target_os = "macos"))]
        let root = root.child(self.render_top_bar(cx));

        #[cfg(target_os = "macos")]
        let root = root
            .when_some(self.controls.as_ref(), |root, controls| {
                root.child(
                    div()
                        .absolute()
                        .top(px(layout::BAR as f32))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(native(controls.scroll.native()).fill()),
                )
                // The zoom, bottom left, level with the add button.
                .when(self.mode == Mode::Canvas, |root| {
                    root.child(
                        div()
                            .absolute()
                            // Over the add button, centred on it; it shows while the zoom
                            // changes and fades out after.
                            .bottom(px((layout::TOOLBAR_SIDE + layout::ADD + 4.) as f32))
                            .right(px(layout::TOOLBAR_SIDE as f32))
                            .w(px(layout::ADD as f32))
                            .flex()
                            .justify_center()
                            .child(native(controls.zoom_label.native())),
                    )
                })
                // The window's main action, bottom right.
                .child(
                    div()
                        .absolute()
                        .bottom(px(layout::TOOLBAR_SIDE as f32))
                        .right(px(layout::TOOLBAR_SIDE as f32))
                        .child(native(controls.add.native())),
                )
            })
            .font_family(".SystemUIFont");
        root
    }
}

/// A soft white fade along every edge of the canvas, like Freeform's.
fn edge_fades() -> [impl IntoElement; 4] {
    let canvas = rgb(palette().canvas);
    let white = linear_color_stop(canvas, 0.);
    let clear = linear_color_stop(Rgba { a: 0., ..canvas }, 1.);
    let edge = || div().absolute();
    [
        edge()
            .top_0()
            .left_0()
            .right_0()
            .h(px(EDGE_FADE))
            .bg(linear_gradient(180., white, clear)),
        edge()
            .bottom_0()
            .left_0()
            .right_0()
            .h(px(EDGE_FADE))
            .bg(linear_gradient(0., white, clear)),
        edge()
            .top_0()
            .bottom_0()
            .left_0()
            .w(px(EDGE_FADE))
            .bg(linear_gradient(90., white, clear)),
        edge()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(EDGE_FADE))
            .bg(linear_gradient(270., white, clear)),
    ]
}

/// The dot grid, zoomed with the canvas like Freeform's: the dots spread and grow as it zooms
/// in, and when they get too close every other one drops out.
/// AppKit's rubber band: fingers `travel` points past an edge move the content this far, never
/// quite `extent`, the size of the view.
fn rubber(travel: f32, extent: f32) -> f32 {
    let c = 0.55;
    let x = travel.abs();
    (1. - 1. / (x * c / extent + 1.)) * extent * travel.signum()
}

/// The fingers' travel that [`rubber`] turns into `offset`.
fn unrubber(offset: f32, extent: f32) -> f32 {
    let c = 0.55;
    let y = offset.abs().min(extent * 0.999);
    (extent / c) * (1. / (1. - y / extent) - 1.) * offset.signum()
}

fn paint_dots(
    bounds: Bounds<Pixels>,
    pan: Point<Pixels>,
    zoom: f32,
    between: f32,
    window: &mut Window,
) {
    let color: Rgba = rgb(palette().dot);
    let mut spacing = DOT_SPACING * zoom;
    while spacing < DOT_SPACING / 2. {
        spacing *= 2.;
    }
    let radius = (DOT_RADIUS * zoom.sqrt()).clamp(0.75, 2.5);
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let dot = size(px(radius * 2.), px(radius * 2.));
    // Index the dots from the canvas origin, so the same ones fade wherever it is panned.
    let first = |pan: f32| (-pan / spacing).ceil() as i64;
    let (first_x, first_y) = (first(f32::from(pan.x)), first(f32::from(pan.y)));
    let mut row = first_y;
    loop {
        let y = f32::from(pan.y) + row as f32 * spacing;
        if y >= height {
            break;
        }
        let mut column = first_x;
        loop {
            let x = f32::from(pan.x) + column as f32 * spacing;
            if x >= width {
                break;
            }
            let odd = column.rem_euclid(2) == 1 || row.rem_euclid(2) == 1;
            let alpha = if odd { between } else { 1. };
            if alpha > 0.01 {
                let origin = bounds.origin + point(px(x - radius), px(y - radius));
                let mut shade = color;
                shade.a *= alpha;
                window.paint_quad(fill(Bounds::new(origin, dot), shade).corner_radii(px(radius)));
            }
            column += 1;
        }
        row += 1;
    }
}

/// How the toolbar shows a box.
#[cfg(target_os = "macos")]
fn box_label(name: &str, icon: &BoxIcon, items: usize) -> ambry_macos::BoxLabel {
    ambry_macos::BoxLabel {
        name: name.to_string(),
        symbol: icon.symbol.clone(),
        color: icon.color.clone(),
        items,
    }
}

/// Holds an animation at its start for the first `start` fraction of its duration.
fn delayed(start: f32, easing: impl Fn(f32) -> f32 + 'static) -> impl Fn(f32) -> f32 {
    move |t| {
        if t <= start {
            0.
        } else {
            easing((t - start) / (1. - start))
        }
    }
}

fn shadow(x: f32, y: f32, blur: f32, alpha: f32) -> BoxShadow {
    BoxShadow {
        color: hsla(0., 0., 0., alpha),
        offset: point(px(x), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
    }
}

/// An item the way Finder's icon view draws it: the file's own icon, or a thumbnail for
/// images, on a grey rounded backdrop while selected.
fn finder_face(
    item: &Item,
    icon: Option<PathBuf>,
    selected: bool,
    lifted: bool,
    scale: f32,
) -> gpui::Div {
    let picture = if item.is_image() {
        Some(item.path.clone())
    } else {
        icon
    };
    let Some(picture) = picture else {
        return card_face(item, selected, lifted, scale);
    };
    div()
        .p(px(4.))
        .rounded(px(6.))
        .when(selected, |this| this.bg(palette().selection))
        .when(lifted, |this| this.opacity(0.8))
        .when(!item.exists(), |this| this.opacity(0.4))
        .child(
            img(picture)
                .size(px(ICON_SIZE * scale))
                .object_fit(ObjectFit::Contain),
        )
}

/// A folder opening under a drag, or closing after it: its own icon gives way to Finder's
/// open folder with a short cross-fade, under a grey highlight as Finder's drop target has.
fn folder_face(closed: PathBuf, open: PathBuf, opening: bool, id: u64, scale: f32) -> gpui::Div {
    let size = px(ICON_SIZE * scale);
    let fade = || Animation::new(Duration::from_millis(OPEN_FADE_MS)).with_easing(ease_out_quint());
    let key = |name: &'static str| (name, id as usize);
    div()
        .relative()
        .p(px(4.))
        .rounded(px(6.))
        .when(opening, |this| this.bg(palette().selection))
        .child(
            img(closed)
                .size(size)
                .object_fit(ObjectFit::Contain)
                .with_animation(
                    key(if opening { "lid-up" } else { "lid-down" }),
                    fade(),
                    move |this, t| this.opacity(if opening { 1. - t } else { t }),
                ),
        )
        .child(
            div()
                .absolute()
                .top(px(4.))
                .left(px(4.))
                .child(img(open).size(size).object_fit(ObjectFit::Contain))
                .with_animation(
                    key(if opening { "open-in" } else { "open-out" }),
                    fade(),
                    move |this, t| this.opacity(if opening { t } else { 1. - t }),
                ),
        )
}

/// Finder's open folder as a PNG in the data directory, rendered the first time.
#[cfg(target_os = "macos")]
fn open_folder_file() -> Option<PathBuf> {
    let dir = ambry_core::data_dir().ok()?.join("icons");
    let file = dir.join(format!("open-folder-{}.png", ICON_SIZE as u32));
    if !file.exists() {
        let png = ambry_macos::finder::open_folder_png(ICON_SIZE.into())?;
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&file, png).ok()?;
    }
    Some(file)
}

#[cfg(not(target_os = "macos"))]
fn open_folder_file() -> Option<PathBuf> {
    None
}

/// Finder's picture for `path` as a PNG in the data directory: the thumbnail Finder's icon
/// view shows, or its plain icon when Quick Look has none. Rendered the first time it is asked
/// for at this `stamp`, the file's modification time, so it follows the file's changes.
#[cfg(target_os = "macos")]
fn finder_icon_file(path: &Path, stamp: Option<std::time::SystemTime>) -> Option<PathBuf> {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    stamp.hash(&mut hasher);
    let dir = ambry_core::data_dir().ok()?.join("icons");
    let file = dir.join(format!("{:016x}.png", hasher.finish()));
    if !file.exists() {
        std::fs::create_dir_all(&dir).ok()?;
        if !ambry_macos::finder::thumbnail_png(path, ICON_SIZE.into(), &file) {
            let png = ambry_macos::finder::icon_png(path, ICON_SIZE.into())?;
            std::fs::write(&file, png).ok()?;
        }
    }
    Some(file)
}

#[cfg(not(target_os = "macos"))]
fn finder_icon_file(_path: &Path, _stamp: Option<std::time::SystemTime>) -> Option<PathBuf> {
    None
}

/// Date modified, size and kind of `path`, written the way Finder's list writes them.
#[cfg(target_os = "macos")]
fn file_details(path: &Path) -> [String; 3] {
    let metadata = std::fs::metadata(path).ok();
    let date = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok())
        .map_or_else(|| "--".to_string(), ambry_macos::finder::date);
    let size = match &metadata {
        Some(metadata) if metadata.is_file() => ambry_macos::finder::size(metadata.len()),
        _ => "--".to_string(),
    };
    let kind = ambry_macos::finder::kind(path).unwrap_or_default();
    [date, size, kind]
}

fn card_face(item: &Item, selected: bool, lifted: bool, scale: f32) -> gpui::Div {
    let border: Hsla = if selected {
        rgb(ACCENT).into()
    } else {
        rgb(palette().hairline).into()
    };
    let face = match item.kind {
        ItemKind::Folder => div()
            .size(px(CARD_HEIGHT * scale))
            .rounded(px(10.))
            .when(selected, |this| this.bg(rgb(palette().pressed)))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path("icons/folder.svg")
                    .size(px(64. * scale))
                    .text_color(rgb(0x5aa0f2)),
            ),
        ItemKind::File => div()
            .relative()
            .w(px(CARD_WIDTH * scale))
            .h(px(CARD_HEIGHT * scale))
            .rounded(px(6.))
            .overflow_hidden()
            .bg(linear_gradient(
                180.,
                linear_color_stop(rgb(palette().card_top), 0.),
                linear_color_stop(rgb(palette().canvas), 1.),
            ))
            .border_1()
            .when(selected, |this| this.border_2())
            .border_color(border)
            .shadow(if lifted {
                vec![shadow(0., 10., 24., 0.12), shadow(0., 2., 6., 0.1)]
            } else {
                vec![shadow(0., 1., 3., 0.06)]
            })
            .when(item.is_image(), |this| {
                this.child(
                    img(item.path.clone())
                        .absolute()
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
            })
            .when(!item.is_image(), |this| {
                this.child(
                    div()
                        .absolute()
                        .right(px(7.))
                        .bottom(px(5.))
                        .text_size(px(LABEL_SIZE))
                        .text_color(rgb(palette().faint))
                        .child(item.badge()),
                )
            }),
    };
    face.when(!item.exists(), |this| this.opacity(0.4))
}

/// The card a file will become, drawn under the cursor while it is dragged in.
fn ghost_face(item: &Item, icon: Option<PathBuf>, scale: f32) -> gpui::AnyElement {
    if let Some(icon) = icon {
        return div()
            .p(px(4.))
            .opacity(0.6)
            .child(
                img(icon)
                    .size(px(ICON_SIZE * scale))
                    .object_fit(ObjectFit::Contain),
            )
            .into_any_element();
    }
    let (width, radius) = match item.kind {
        ItemKind::Folder => (CARD_HEIGHT * scale, 10.),
        ItemKind::File => (CARD_WIDTH * scale, 6.),
    };
    div()
        .w(px(width))
        .h(px(CARD_HEIGHT * scale))
        .rounded(px(radius))
        .border_1()
        .border_dashed()
        .border_color(rgb(ACCENT))
        .bg(hsla(216. / 360., 0.92, 0.57, 0.08))
        .flex()
        .items_end()
        .justify_end()
        .p(px(6.))
        .text_size(px(LABEL_SIZE))
        .text_color(rgb(ACCENT))
        .child(item.badge())
        .into_any_element()
}

fn item_label(name: String, selected: bool, exists: bool) -> gpui::Div {
    div()
        .max_w(px(ITEM_WIDTH))
        .px(px(6.))
        .rounded(px(4.))
        .when(selected, |this| this.bg(rgb(ACCENT)))
        .when(!exists, |this| this.opacity(0.4))
        .text_size(px(LABEL_SIZE))
        .text_color(if selected {
            gpui::white()
        } else {
            rgb(palette().quiet).into()
        })
        .truncate()
        .child(name)
}

fn empty_hint() -> impl IntoElement {
    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(TITLE_SIZE))
        .text_color(rgb(palette().faint))
        .child("Drop files or folders here")
}

#[cfg(not(target_os = "macos"))]
fn floating() -> gpui::Div {
    div()
        .rounded_full()
        .bg(rgb(palette().canvas))
        .shadow(vec![
            shadow(0., 4., 16., 0.06),
            shadow(0., 1., 4., 0.17),
            shadow(0., 0., 1., 0.3),
        ])
        .occlude()
}

fn user_initial() -> SharedString {
    #[cfg(target_os = "macos")]
    let name = Some(ambry_macos::settings::user_name());
    #[cfg(not(target_os = "macos"))]
    let name = std::env::var("USER").ok();
    name.and_then(|user| user.chars().next())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "A".into())
        .into()
}

/// Where the account picture is kept.
fn avatar_path() -> Option<PathBuf> {
    ambry_core::data_dir()
        .ok()
        .map(|dir| dir.join("avatar.png"))
}

/// The view boxes open in: the list when Settings says so.
fn default_mode() -> Mode {
    #[cfg(target_os = "macos")]
    if ambry_macos::settings::opens_in_list() {
        return Mode::Columns;
    }
    Mode::Canvas
}
