//! The box window: a dotted canvas of file cards, a list view, the title bar and the toolbar.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use ambry_core::{BoxDoc, DROP_STEP_X, Item, ItemKind, Store};
use ambry_macos::layout;
use gpui::{
    Animation, AnimationExt as _, App, Bounds, BoxShadow, Context, CursorStyle, DragMoveEvent,
    ExternalPaths, FocusHandle, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    ObjectFit, Pixels, Point, Rgba, ScrollWheelEvent, SharedString, Window, canvas, div,
    ease_out_quint, fill, hsla, img, linear_color_stop, linear_gradient, point, prelude::*, px,
    rgb, size, svg,
};

const TITLEBAR_HEIGHT: f32 = 52.;
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

const INK: u32 = 0x1d1d1f;
const QUIET: u32 = 0x8e8e93;
const FAINT: u32 = 0xb4b4ba;
const DOT: u32 = 0xe3e3e8;
const HAIRLINE: u32 = 0xe5e5ea;
const PRESSED: u32 = 0xececf0;
const ACCENT: u32 = 0x2f7cf6;

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
    #[cfg(target_os = "macos")]
    toolbar: Option<ambry_macos::Toolbar>,
}

impl BoxView {
    pub fn new(doc: BoxDoc, store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
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
            avatar,
            focus,
            #[cfg(target_os = "macos")]
            toolbar: Self::attach_toolbar(window, cx),
        }
    }

    #[cfg(target_os = "macos")]
    fn attach_toolbar(window: &mut Window, cx: &mut Context<Self>) -> Option<ambry_macos::Toolbar> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        let (events, received) = async_channel::unbounded();
        // SAFETY: GPUI hands out its own live content view and we are on the main thread.
        let toolbar = unsafe {
            ambry_macos::Toolbar::attach(appkit.ns_view, move |event| {
                events.try_send(event).ok();
            })
        }?;
        if std::env::var_os("AMBRY_SWITCH_GALLERY").is_some() {
            // SAFETY: as above.
            unsafe { ambry_macos::Toolbar::attach_switch_gallery(appkit.ns_view) };
        }
        cx.spawn(async move |this, cx| {
            while let Ok(event) = received.recv().await {
                let handled = this.update(cx, |this, cx| match event {
                    ambry_macos::ToolbarEvent::Mode(index) => {
                        this.set_mode(if index == 0 { Mode::Canvas } else { Mode::List }, cx)
                    }
                    ambry_macos::ToolbarEvent::Add => this.pick_paths(cx),
                });
                if handled.is_err() {
                    break;
                }
            }
        })
        .detach();
        Some(toolbar)
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        #[cfg(target_os = "macos")]
        if let Some(toolbar) = &self.toolbar {
            toolbar.set_mode(if mode == Mode::Canvas { 0 } else { 1 });
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
            f32::from(position.y - self.pan.y) - TITLEBAR_HEIGHT,
        )
    }

    /// Top-left corner of an item in window coordinates.
    fn item_origin(&self, item: &Item) -> Point<Pixels> {
        point(
            self.pan.x + px(item.x),
            self.pan.y + px(item.y + TITLEBAR_HEIGHT),
        )
    }

    /// Where the first of `count` dropped items lands: centred under the cursor,
    /// or in the middle of the visible canvas when there is no cursor position.
    fn drop_origin(&self, count: usize, at: Option<Point<Pixels>>) -> (f32, f32) {
        let row_width = count.saturating_sub(1) as f32 * DROP_STEP_X;
        let position = match (self.mode, at) {
            (Mode::Canvas, Some(position)) => position,
            _ => point(
                self.viewport.width / 2.,
                self.viewport.height / 2. + px(TITLEBAR_HEIGHT / 2.),
            ),
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

    fn render_titlebar(&self) -> impl IntoElement {
        div()
            .id("titlebar")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(TITLEBAR_HEIGHT))
            .bg(gpui::white())
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .text_size(px(TITLE_SIZE))
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(
                div()
                    .size(px(20.))
                    .rounded(px(5.))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(rgb(0x4c9bff), 0.),
                        linear_color_stop(rgb(ACCENT), 1.),
                    ))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path("icons/frame.svg")
                            .size(px(13.))
                            .text_color(gpui::white()),
                    ),
            )
            .child(
                div()
                    .text_color(rgb(INK))
                    .child(SharedString::from(self.doc.name.clone())),
            )
            .child(div().text_color(rgb(FAINT)).child(self.status()))
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
            .top(px(TITLEBAR_HEIGHT))
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
            .child(card_face(item, selected, lifted))
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
                    .child(ghost_face(item))
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

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("list")
            .absolute()
            .top(px(TITLEBAR_HEIGHT))
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
                    .when(selected, |this| this.bg(rgb(PRESSED)))
                    .when(!item.exists(), |this| this.opacity(0.4))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.on_item_mouse_down(id, event, window, cx)
                        }),
                    )
                    .child(div().w(px(52.)).text_color(rgb(FAINT)).child(item.badge()))
                    .child(div().text_color(rgb(INK)).child(item.name()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(rgb(FAINT))
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

    fn render_avatar(&self) -> impl IntoElement {
        let size = px(layout::AVATAR as f32);
        div()
            .id("account")
            .absolute()
            .left(px(layout::INSET as f32))
            .bottom(px(layout::INSET as f32))
            .size(size)
            .rounded_full()
            .bg(gpui::white())
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
            .text_color(rgb(INK))
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
                .when(active, |this| this.bg(rgb(PRESSED)))
                .child(svg().path(icon).size(px(18.)).text_color(if active {
                    rgb(INK)
                } else {
                    rgb(QUIET)
                }))
        };
        div()
            .absolute()
            .left(px(
                (layout::INSET + layout::AVATAR + layout::GAP_AFTER_AVATAR) as f32,
            ))
            .bottom(px(layout::INSET as f32))
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
                            .text_color(rgb(INK)),
                    ),
            )
    }
}

impl Render for BoxView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.viewport = window.viewport_size();
        if !cx.has_active_drag() {
            self.drop_preview = None;
        }

        let root = div()
            .id("box")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(gpui::white())
            .text_color(rgb(INK))
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
            .child(self.render_titlebar())
            .child(self.render_avatar());

        #[cfg(target_os = "macos")]
        let root = root.font_family(".SystemUIFont");
        #[cfg(not(target_os = "macos"))]
        let root = root.child(self.render_fallback_toolbar(cx));
        root
    }
}

fn paint_dots(bounds: Bounds<Pixels>, pan: Point<Pixels>, window: &mut Window) {
    let color: Rgba = rgb(DOT);
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
    let white = linear_color_stop(gpui::white(), 0.);
    let clear = linear_color_stop(hsla(0., 0., 1., 0.), 1.);
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

fn card_face(item: &Item, selected: bool, lifted: bool) -> gpui::Div {
    let border: Hsla = if selected {
        rgb(ACCENT).into()
    } else {
        rgb(HAIRLINE).into()
    };
    let face = match item.kind {
        ItemKind::Folder => div()
            .size(px(CARD_HEIGHT))
            .rounded(px(10.))
            .when(selected, |this| this.bg(rgb(PRESSED)))
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
                linear_color_stop(rgb(0xf2f2f5), 0.),
                linear_color_stop(gpui::white(), 1.),
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
                        .text_color(rgb(FAINT))
                        .child(item.badge()),
                )
            }),
    };
    face.when(!item.exists(), |this| this.opacity(0.4))
}

/// The card a file will become, drawn under the cursor while it is dragged in.
fn ghost_face(item: &Item) -> impl IntoElement {
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
            rgb(QUIET).into()
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
        .text_color(rgb(FAINT))
        .child("Drop files or folders here")
}

#[cfg(not(target_os = "macos"))]
fn floating() -> gpui::Div {
    div()
        .rounded_full()
        .bg(gpui::white())
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
