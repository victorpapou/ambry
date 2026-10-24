//! The box window: a dotted canvas of files drawn like Finder's icon view, a list view and the
//! bottom bar. On macOS the list, the avatar and the bottom bar controls are AppKit views.

use std::collections::HashMap;
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
/// The list's rows and their inset from the window's sides.
#[cfg(target_os = "macos")]
const LIST_ROW: f32 = 28.;
#[cfg(target_os = "macos")]
const LIST_ROW_PAD: f32 = 10.;
#[cfg(target_os = "macos")]
const LIST_SIDE: f32 = 10.;
#[cfg(target_os = "macos")]
const LIST_HEADER: f32 = 28.;
#[cfg(target_os = "macos")]
const LIST_ICON: f32 = 18.;
/// How long a list row's hover highlight takes to fade in or out, in milliseconds.
#[cfg(target_os = "macos")]
const ROW_FADE: u64 = 180;

const ACCENT: u32 = 0x2f7cf6;

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
    List,
}

struct Drag {
    id: u64,
    /// Mouse position minus the item's top-left corner, in window coordinates.
    grab: Point<Pixels>,
    moved: bool,
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
    selected: Option<u64>,
    drag: Option<Drag>,
    drop_preview: Option<DropPreview>,
    /// Seconds to wait before a card plays its appear animation, so a batch lands one by one.
    appear_delay: HashMap<u64, f32>,
    viewport: gpui::Size<Pixels>,
    avatar: Option<PathBuf>,
    focus: FocusHandle,
    /// Finder's icon for each item path, rendered once into the data directory.
    icons: HashMap<PathBuf, Option<PathBuf>>,
    /// Date modified, size and kind of each item path, as the list shows them.
    #[cfg(target_os = "macos")]
    details: HashMap<PathBuf, [String; 3]>,
    #[cfg(target_os = "macos")]
    controls: Option<NativeControls>,
    /// The list row under the pointer, and the row it last left.
    #[cfg(target_os = "macos")]
    hovered_row: Option<u64>,
    #[cfg(target_os = "macos")]
    left_row: Option<u64>,
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
    _toolbar_blur: ambry_macos::SoftEdge,
    /// AppKit's overlay scroll bars along the canvas's right and bottom edges.
    scroll: ambry_macos::CanvasScrollView,
    /// The add button, bottom right.
    add: ambry_macos::GlassButton,
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
    /// A scroll bar moved the visible part of the canvas to this top-left, counted from the
    /// top-left of [`BoxView::scroll_area`].
    Scroll(f64, f64),
    /// Quick Look moved to the item at this index.
    Preview(usize),
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
            selected: None,
            drag: None,
            drop_preview: None,
            appear_delay,
            viewport: window.viewport_size(),
            #[cfg(target_os = "macos")]
            controls: Self::attach_controls(boxes_dir, window, cx),
            avatar,
            focus,
            icons: HashMap::new(),
            #[cfg(target_os = "macos")]
            details: HashMap::new(),
            #[cfg(target_os = "macos")]
            hovered_row: None,
            #[cfg(target_os = "macos")]
            left_row: None,
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
                _toolbar_blur: toolbar_blur,
                scroll,
                add,
                shown_scroll: None,
                events: previewed,
            }
        };
        cx.spawn(async move |this, cx| {
            while let Ok(event) = received.recv().await {
                let handled = this.update(cx, |this, cx| match event {
                    ControlEvent::Mode(index) => {
                        this.set_mode(if index == 0 { Mode::Canvas } else { Mode::List }, cx)
                    }
                    ControlEvent::Add => this.pick_paths(cx),
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
        let Some(controls) = &mut self.controls else {
            return;
        };
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
    }

    /// Renders Finder's icon for every item that has none yet.
    fn load_icons(&mut self) {
        let paths = self
            .doc
            .items
            .iter()
            .chain(self.drop_preview.iter().flat_map(|preview| &preview.items))
            .filter(|item| !item.is_image())
            .map(|item| item.path.clone());
        for path in paths {
            self.icons
                .entry(path)
                .or_insert_with_key(|path| finder_icon_file(path));
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
        self.drag = None;
        self.pan = point(px(0.), px(0.));
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
        for item in &self.doc.items {
            area.0 = area.0.min(item.x - CANVAS_MARGIN);
            area.1 = area.1.min(item.y - CANVAS_MARGIN);
            area.2 = area.2.max(item.x + ITEM_WIDTH + CANVAS_MARGIN);
            area.3 = area.3.max(item.y + ITEM_HEIGHT + CANVAS_MARGIN);
        }
        area
    }

    /// Keeps the view inside [`Self::scroll_area`].
    fn clamp_pan(&mut self) {
        let area = self.scroll_area();
        let clamp = |pan: Pixels, min: f32, max: f32, visible: Pixels| {
            let at = -f32::from(pan);
            let at = at.min(max - f32::from(visible)).max(min);
            px(-at)
        };
        self.pan = point(
            clamp(self.pan.x, area.0, area.2, self.viewport.width),
            clamp(self.pan.y, area.1, area.3, self.viewport.height),
        );
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
            f32::from(position.x - self.pan.x),
            f32::from(position.y - self.pan.y),
        )
    }

    /// Top-left corner of an item in window coordinates.
    fn item_origin(&self, item: &Item) -> Point<Pixels> {
        point(self.pan.x + px(item.x), self.pan.y + px(item.y))
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
                });
            }
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = &self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_drag(cx);
            return;
        }
        let id = drag.id;
        let (x, y) = self.to_canvas(event.position - drag.grab);
        self.doc.move_item(id, x, y);
        if let Some(drag) = &mut self.drag {
            drag.moved = true;
        }
        cx.notify();
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.take() {
            if drag.moved {
                self.save();
            }
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "backspace" | "delete" => {
                if let Some(id) = self.selected.take() {
                    self.doc.remove(id);
                    self.save();
                    cx.notify();
                }
            }
            "escape" => {
                self.selected = None;
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
        let pan = self.pan;
        let mut items: Vec<gpui::AnyElement> = self
            .doc
            .items
            .iter()
            .map(|item| self.render_canvas_item(item, cx).into_any_element())
            .collect();
        if let Some(preview) = &self.drop_preview {
            items.extend(self.render_ghosts(preview));
        }

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
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    this.selected = None;
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                this.scroll_by(event.delta.pixel_delta(window.line_height()), cx);
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| paint_dots(bounds, pan, window),
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
        let selected = self.selected == Some(id);
        let lifted = self
            .drag
            .as_ref()
            .is_some_and(|drag| drag.id == id && drag.moved);
        let left = self.pan.x + px(item.x);
        let top = self.pan.y + px(item.y) - if lifted { px(3.) } else { px(0.) };
        let delay = self.appear_delay.get(&id).copied().unwrap_or(0.);

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
            .child(finder_face(item, self.icon(item), selected, lifted))
            .child(item_label(item.name(), selected, item.exists()))
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
                let top = self.pan.y + px(y);
                div()
                    .absolute()
                    .left(self.pan.x + px(x + index as f32 * DROP_STEP_X))
                    .top(top)
                    .w(px(ITEM_WIDTH))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .child(ghost_face(item, self.icon(item)))
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
        self.icons.get(&item.path).cloned().flatten()
    }

    /// Notes the row under the pointer, and the one it left, whose highlight fades out.
    #[cfg(target_os = "macos")]
    fn hover_row(&mut self, id: u64, inside: bool, cx: &mut Context<Self>) {
        if inside {
            if self.hovered_row == Some(id) {
                return;
            }
            // The row left may already have reported it; keep its fade-out then.
            if let Some(previous) = self.hovered_row.replace(id) {
                self.left_row = Some(previous);
            }
        } else if self.hovered_row == Some(id) {
            self.hovered_row = None;
            self.left_row = Some(id);
        } else {
            return;
        }
        cx.notify();
    }

    /// The grey highlight of a row under the pointer, fading in, or of the row it just left,
    /// fading out.
    #[cfg(target_os = "macos")]
    fn row_hover(&self, id: u64) -> Option<gpui::AnyElement> {
        let entering = self.hovered_row == Some(id);
        if !entering && self.left_row != Some(id) {
            return None;
        }
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .rounded(px(7.))
                .bg(rgb(palette().pressed))
                .with_animation(
                    (
                        if entering {
                            "row-hover-in"
                        } else {
                            "row-hover-out"
                        },
                        id as usize,
                    ),
                    Animation::new(Duration::from_millis(ROW_FADE)).with_easing(ease_out_quint()),
                    move |this, t| this.opacity(if entering { t } else { 1. - t }),
                )
                .into_any_element(),
        )
    }

    /// The list: Finder's columns, drawn by GPUI, with rounded rows that fill with the accent
    /// color when selected.
    #[cfg(target_os = "macos")]
    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let header = |text: &'static str| div().text_color(rgb(palette().quiet)).child(text);
        div()
            .id("list")
            .absolute()
            .top(px(layout::BAR as f32))
            .left_0()
            .right_0()
            .bottom_0()
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
            .child(
                list_columns(
                    div()
                        .flex_none()
                        .h(px(LIST_HEADER))
                        .mx(px(LIST_SIDE))
                        .px(px(LIST_ROW_PAD))
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::MEDIUM),
                    header("Name"),
                    header("Date Modified"),
                    header("Size"),
                    header("Kind"),
                )
                .border_b_1()
                .border_color(rgb(palette().hairline)),
            )
            .child(
                div()
                    .id("list-rows")
                    .flex_1()
                    .overflow_y_scroll()
                    .px(px(LIST_SIDE))
                    .pt(px(6.))
                    // Room for the last rows to scroll out from under the add button.
                    .pb(px((layout::ADD + layout::ADD_MARGIN * 2.) as f32))
                    .flex()
                    .flex_col()
                    .when(self.doc.items.is_empty(), |this| this.child(empty_hint()))
                    .children(
                        self.doc
                            .items
                            .iter()
                            .enumerate()
                            .map(|(index, item)| {
                                self.render_list_row(index, item, cx).into_any_element()
                            })
                            .collect::<Vec<_>>(),
                    ),
            )
    }

    #[cfg(target_os = "macos")]
    fn render_list_row(
        &self,
        index: usize,
        item: &Item,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = item.id;
        let selected = self.selected == Some(id);
        let (ink, quiet): (Hsla, Hsla) = if selected {
            (rgb(0xffffff).into(), hsla(0., 0., 1., 0.8))
        } else {
            (rgb(palette().ink).into(), rgb(palette().quiet).into())
        };
        let icon = self
            .icon(item)
            .or_else(|| item.is_image().then(|| item.path.clone()));
        let blank = [String::new(), String::new(), String::new()];
        let [date, size, kind] = self.details.get(&item.path).unwrap_or(&blank).clone();
        let row = div()
            .id(("row", id as usize))
            .flex_none()
            .h(px(LIST_ROW))
            .px(px(LIST_ROW_PAD))
            .rounded(px(7.))
            .when(selected, |this| this.bg(rgb(ACCENT)))
            .when(!selected && index % 2 == 1, |this| {
                this.bg(rgb(palette().stripe))
            })
            .relative()
            .when(!selected, |this| this.children(self.row_hover(id)))
            .when(!item.exists(), |this| this.opacity(0.4))
            .on_hover(
                cx.listener(move |this, inside: &bool, _, cx| this.hover_row(id, *inside, cx)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, window, cx| {
                    this.on_item_mouse_down(id, event, window, cx)
                }),
            );
        list_columns(
            row,
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(match icon {
                    Some(path) => img(path)
                        .flex_none()
                        .size(px(LIST_ICON))
                        .object_fit(ObjectFit::Contain)
                        .into_any_element(),
                    None => div().flex_none().size(px(LIST_ICON)).into_any_element(),
                })
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(ink)
                        .child(item.name()),
                ),
            div().truncate().text_color(quiet).child(date),
            div().text_color(quiet).child(size),
            div().truncate().text_color(quiet).child(kind),
        )
        .with_animation(
            ("row-appear", id as usize),
            Animation::new(Duration::from_millis(260 + 20 * index.min(12) as u64))
                .with_easing(ease_out_quint()),
            |this, t| this.opacity(t),
        )
    }

    #[cfg(not(target_os = "macos"))]
    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                        mode_button("mode-list", "icons/list.svg", Mode::List)
                            .on_click(cx.listener(|this, _, _, cx| this.set_mode(Mode::List, cx))),
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
        self.viewport = window.viewport_size();
        if self.drag.is_none() {
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
                Mode::List => this.child(self.render_list(cx)),
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
                // The window's main action, bottom right.
                .child(
                    div()
                        .absolute()
                        .bottom(px(layout::ADD_MARGIN as f32))
                        .right(px(layout::ADD_MARGIN as f32))
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

fn paint_dots(bounds: Bounds<Pixels>, pan: Point<Pixels>, window: &mut Window) {
    let color: Rgba = rgb(palette().dot);
    let start_x = f32::from(pan.x).rem_euclid(DOT_SPACING);
    let start_y = f32::from(pan.y).rem_euclid(DOT_SPACING);
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let dot = size(px(DOT_RADIUS * 2.), px(DOT_RADIUS * 2.));

    let mut y = start_y;
    while y < height {
        let mut x = start_x;
        while x < width {
            let origin = bounds.origin + point(px(x - DOT_RADIUS), px(y - DOT_RADIUS));
            window.paint_quad(fill(Bounds::new(origin, dot), color).corner_radii(px(DOT_RADIUS)));
            x += DOT_SPACING;
        }
        y += DOT_SPACING;
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
fn finder_face(item: &Item, icon: Option<PathBuf>, selected: bool, lifted: bool) -> gpui::Div {
    let picture = if item.is_image() {
        Some(item.path.clone())
    } else {
        icon
    };
    let Some(picture) = picture else {
        return card_face(item, selected, lifted);
    };
    div()
        .p(px(4.))
        .rounded(px(6.))
        .when(selected, |this| this.bg(palette().selection))
        .when(lifted, |this| this.opacity(0.8))
        .when(!item.exists(), |this| this.opacity(0.4))
        .child(
            img(picture)
                .size(px(ICON_SIZE))
                .object_fit(ObjectFit::Contain),
        )
}

/// Finder's icon for `path` as a PNG in the data directory, rendered the first time it is
/// asked for.
#[cfg(target_os = "macos")]
fn finder_icon_file(path: &Path) -> Option<PathBuf> {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let dir = ambry_core::data_dir().ok()?.join("icons");
    let file = dir.join(format!("{:016x}.png", hasher.finish()));
    if !file.exists() {
        let png = ambry_macos::finder::icon_png(path, ICON_SIZE.into())?;
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&file, png).ok()?;
    }
    Some(file)
}

#[cfg(not(target_os = "macos"))]
fn finder_icon_file(_path: &Path) -> Option<PathBuf> {
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

/// Lays out a list row's cells in Finder's columns: the name takes the room left.
#[cfg(target_os = "macos")]
fn list_columns(
    row: impl IntoElement + ParentElement + Styled,
    name: impl IntoElement + Styled,
    date: impl IntoElement + Styled,
    size: impl IntoElement + Styled,
    kind: impl IntoElement + Styled,
) -> impl IntoElement + ParentElement + Styled {
    row.flex()
        .items_center()
        .gap(px(16.))
        .child(name.flex_1().min_w_0())
        .child(date.flex_none().w(px(170.)))
        .child(size.flex_none().w(px(72.)))
        .child(kind.flex_none().w(px(150.)))
}

fn card_face(item: &Item, selected: bool, lifted: bool) -> gpui::Div {
    let border: Hsla = if selected {
        rgb(ACCENT).into()
    } else {
        rgb(palette().hairline).into()
    };
    let face = match item.kind {
        ItemKind::Folder => div()
            .size(px(CARD_HEIGHT))
            .rounded(px(10.))
            .when(selected, |this| this.bg(rgb(palette().pressed)))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path("icons/folder.svg")
                    .size(px(64.))
                    .text_color(rgb(0x5aa0f2)),
            ),
        ItemKind::File => div()
            .relative()
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
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
fn ghost_face(item: &Item, icon: Option<PathBuf>) -> gpui::AnyElement {
    if let Some(icon) = icon {
        return div()
            .p(px(4.))
            .opacity(0.6)
            .child(img(icon).size(px(ICON_SIZE)).object_fit(ObjectFit::Contain))
            .into_any_element();
    }
    let (width, radius) = match item.kind {
        ItemKind::Folder => (CARD_HEIGHT, 10.),
        ItemKind::File => (CARD_WIDTH, 6.),
    };
    div()
        .w(px(width))
        .h(px(CARD_HEIGHT))
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

fn item_label(name: String, selected: bool, exists: bool) -> impl IntoElement {
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
        return Mode::List;
    }
    Mode::Canvas
}
