//! The box window: a dotted canvas of files drawn like Finder's icon view, a list view and the
//! bottom bar. On macOS the list, the avatar and the bottom bar controls are AppKit views.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ambry_core::{BoxDoc, DROP_STEP_X, Item, ItemKind, Store};
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

/// Width of each canvas/list tab in the native switch, in points.
#[cfg(target_os = "macos")]
const SWITCH_ITEM_WIDTH: f64 = 40.;
/// Finder icons on the canvas are this many points wide.
const ICON_SIZE: f32 = 64.;
const DOT_SPACING: f32 = 20.;
const DOT_RADIUS: f32 = 1.5;
const CARD_WIDTH: f32 = 64.;
const CARD_HEIGHT: f32 = 80.;
/// Width of a card together with its file name underneath.
const ITEM_WIDTH: f32 = 128.;
/// Width of the white fade along the canvas edges.
const EDGE_FADE: f32 = 40.;
const TITLE_SIZE: f32 = 15.;
const LABEL_SIZE: f32 = 13.;

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
}

const LIGHT: Palette = Palette {
    canvas: 0xffffff,
    ink: 0x1d1d1f,
    quiet: 0x8e8e93,
    faint: 0xb4b4ba,
    dot: 0xe3e3e8,
    hairline: 0xe5e5ea,
    pressed: 0xececf0,
    card_top: 0xf2f2f5,
    selection: Hsla {
        h: 0.,
        s: 0.,
        l: 0.,
        a: 0.08,
    },
};

const DARK: Palette = Palette {
    canvas: 0x1e1e1e,
    ink: 0xf5f5f7,
    quiet: 0x98989d,
    faint: 0x6e6e73,
    dot: 0x3a3a3c,
    hairline: 0x3a3a3c,
    pressed: 0x2c2c2e,
    card_top: 0x2c2c2e,
    selection: Hsla {
        h: 0.,
        s: 0.,
        l: 1.,
        a: 0.12,
    },
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
    #[cfg(target_os = "macos")]
    controls: Option<NativeControls>,
}

/// The AppKit views of the window. GPUI lays each out through a `native()` element.
#[cfg(target_os = "macos")]
struct NativeControls {
    ns_view: std::ptr::NonNull<std::ffi::c_void>,
    table: ambry_macos::FileTable,
    /// The paths the table shows, to reload it only when the box changes.
    table_paths: Vec<PathBuf>,
    avatar: ambry_macos::GlassAvatar,
    switch: ambry_macos::TabSwitch,
    add: ambry_macos::GlassButton,
    subtitle: String,
    /// Overrides for the glass look, read from `target/glass-tune.txt` while it exists.
    tune: String,
    /// The look last applied, to touch AppKit only when it changes.
    look: Option<(bool, String)>,
}

#[cfg(target_os = "macos")]
enum ControlEvent {
    Mode(usize),
    Add,
    Open(usize),
    Select(Option<usize>),
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
        let avatar = ambry_core::data_dir()
            .ok()
            .map(|dir| dir.join("avatar.png"))
            .filter(|path| path.exists());
        Self {
            doc,
            store,
            mode: Mode::Canvas,
            pan: point(px(0.), px(0.)),
            selected: None,
            drag: None,
            drop_preview: None,
            appear_delay,
            viewport: window.viewport_size(),
            #[cfg(target_os = "macos")]
            controls: Self::attach_controls(avatar.as_deref(), window, cx),
            avatar,
            focus,
            icons: HashMap::new(),
        }
    }

    #[cfg(target_os = "macos")]
    fn attach_controls(
        avatar: Option<&Path>,
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
        // The table goes in first so the bottom bar floats above it.
        let controls = unsafe {
            let open = events.clone();
            let select = events.clone();
            let table = ambry_macos::FileTable::new(
                ns_view,
                move |row| {
                    open.try_send(ControlEvent::Open(row)).ok();
                },
                move |row| {
                    select.try_send(ControlEvent::Select(row)).ok();
                },
            )?;
            table.set_bottom_inset(layout::INSET * 2. + layout::HEIGHT);
            table.native().set_hidden(true);
            let avatar =
                ambry_macos::GlassAvatar::new(ns_view, avatar, &user_initial(), layout::AVATAR)?;
            let mode = events.clone();
            let switch = ambry_macos::TabSwitch::new(
                ns_view,
                &[("number", "Canvas"), ("list.bullet", "List")],
                &[],
                SWITCH_ITEM_WIDTH,
                move |event| {
                    if let ambry_macos::TabEvent::Select(index) = event {
                        mode.try_send(ControlEvent::Mode(index)).ok();
                    }
                },
            )?;
            let add = ambry_macos::GlassButton::new(
                ns_view,
                "plus",
                "Add files or folders",
                layout::HEIGHT,
                move || {
                    events.try_send(ControlEvent::Add).ok();
                },
            )?;
            NativeControls {
                ns_view,
                table,
                table_paths: Vec::new(),
                avatar,
                switch,
                add,
                subtitle: String::new(),
                tune: String::new(),
                look: None,
            }
        };
        cx.spawn(async move |this, cx| {
            let Some(file) = tune_file() else { return };
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let tune = std::fs::read_to_string(&file).unwrap_or_default();
                let updated = this.update(cx, |this, cx| {
                    if let Some(controls) = &mut this.controls {
                        if controls.tune != tune {
                            controls.tune = tune;
                            cx.notify();
                        }
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = received.recv().await {
                let handled = this.update(cx, |this, cx| match event {
                    ControlEvent::Mode(index) => {
                        this.set_mode(if index == 0 { Mode::Canvas } else { Mode::List }, cx)
                    }
                    ControlEvent::Add => this.pick_paths(cx),
                    ControlEvent::Open(row) => {
                        if let Some(id) = this.doc.items.get(row).map(|item| item.id) {
                            this.open_item(id, cx);
                        }
                    }
                    ControlEvent::Select(row) => {
                        this.selected = row
                            .and_then(|row| this.doc.items.get(row))
                            .map(|item| item.id);
                        cx.notify();
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
        let status = self.status().to_string();
        let Some(controls) = &mut self.controls else {
            return;
        };
        let selected = self
            .selected
            .and_then(|id| self.doc.items.iter().position(|item| item.id == id));
        let paths: Vec<PathBuf> = self
            .doc
            .items
            .iter()
            .map(|item| item.path.clone())
            .collect();
        if controls.table_paths != paths {
            let rows: Vec<ambry_macos::FileRow> = self
                .doc
                .items
                .iter()
                .map(|item| ambry_macos::FileRow {
                    path: item.path.clone(),
                    name: item.name(),
                })
                .collect();
            controls.table.set_rows(&rows, selected);
            controls.table_paths = paths;
        } else {
            controls.table.select(selected);
        }
        controls.table.native().set_hidden(self.mode != Mode::List);
        let look = (DARK_MODE.get(), controls.tune.clone());
        if controls.look.as_ref() != Some(&look) {
            apply_glass_look(controls, look.0, &look.1);
            controls.look = Some(look);
        }
        if controls.subtitle != status {
            // SAFETY: the content view lives as long as the window, and so does this view.
            unsafe { ambry_macos::set_window_subtitle(controls.ns_view, &status) };
            controls.subtitle = status;
        }
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
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        #[cfg(target_os = "macos")]
        if let Some(controls) = &self.controls {
            controls
                .switch
                .select(if mode == Mode::Canvas { 0 } else { 1 });
        }
        cx.notify();
    }

    fn save(&self) {
        if let Err(err) = self.store.save(&self.doc) {
            eprintln!("could not save {}: {err:#}", self.store.path().display());
        }
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
            _ => {}
        }
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
                this.pan = this.pan + event.delta.pixel_delta(window.line_height());
                cx.notify();
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

    /// The list: AppKit's table with Finder's columns.
    #[cfg(target_os = "macos")]
    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = cx;
        div()
            .id("list")
            .absolute()
            .size_full()
            .when_some(self.controls.as_ref(), |this, controls| {
                this.child(native(controls.table.native()).fill())
            })
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

    /// The account avatar, the canvas/list switch and the add button.
    fn render_bottom_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let bar = div()
            .absolute()
            .left(px(layout::INSET as f32))
            .bottom(px(layout::INSET as f32))
            .flex()
            .items_center()
            .gap(px(layout::GAP_AFTER_AVATAR as f32));
        #[cfg(target_os = "macos")]
        let bar = bar.when_some(self.controls.as_ref(), |bar, controls| {
            let _ = cx;
            bar.child(native(controls.avatar.native())).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(layout::GAP as f32))
                    .child(native(controls.switch.native()))
                    .child(native(controls.add.native())),
            )
        });
        #[cfg(not(target_os = "macos"))]
        let bar = bar
            .child(self.render_avatar())
            .child(self.render_fallback_toolbar(cx));
        bar
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
            })
            .child(self.render_bottom_bar(cx));

        #[cfg(target_os = "macos")]
        let root = root.font_family(".SystemUIFont");
        root
    }
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

/// Tints the glass controls so they read as one set in either appearance. Lines like
/// `dark.tabs = 0 0.2` in `tune` (white, alpha, or `none`) override the defaults.
#[cfg(target_os = "macos")]
fn apply_glass_look(controls: &NativeControls, dark: bool, tune: &str) {
    let theme = if dark { "dark" } else { "light" };
    let value = |key: &str| {
        tune.lines().find_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name.trim() == format!("{theme}.{key}")).then(|| value.trim().to_string())
        })
    };
    let tint = |key: &str, default: Option<(f64, f64)>| match value(key) {
        Some(value) if value == "none" => None,
        Some(value) => {
            let mut numbers = value.split_whitespace().filter_map(|n| n.parse().ok());
            numbers.next().zip(numbers.next()).or(default)
        }
        None => default,
    };
    let strip = value("strip")
        .and_then(|value| value.parse().ok())
        .unwrap_or(1.);
    controls.switch.set_look(tint("tabs", None), strip);
    controls.add.set_tint(tint("add", None));
    controls.avatar.set_tint(tint("avatar", None));
}

/// `target/glass-tune.txt` in the checkout the app was built from.
#[cfg(target_os = "macos")]
fn tune_file() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let root = exe
        .ancestors()
        .find(|dir| dir.join("Cargo.toml").exists())?;
    Some(root.join("target/glass-tune.txt"))
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
    std::env::var("USER")
        .ok()
        .and_then(|user| user.chars().next())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "A".into())
        .into()
}
