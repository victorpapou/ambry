//! The box window: a dotted canvas of file cards, a list view, the title bar and the toolbar.

use std::path::PathBuf;

use ambry_core::{BoxDoc, Item, ItemKind, Store};
use gpui::{
    App, Bounds, Context, ExternalPaths, FocusHandle, Hsla, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, ObjectFit, PathPromptOptions, Pixels, Point, Rgba,
    ScrollWheelEvent, SharedString, Window, canvas, div, fill, img, point, prelude::*, px, rgb,
    size, svg,
};

const TITLEBAR_HEIGHT: f32 = 52.;
const DOT_SPACING: f32 = 20.;
const DOT_RADIUS: f32 = 1.25;
const CARD_WIDTH: f32 = 64.;
const CARD_HEIGHT: f32 = 80.;
/// Width of a card together with its file name underneath.
const ITEM_WIDTH: f32 = 128.;

const INK: u32 = 0x1d1d1f;
const QUIET: u32 = 0x8e8e93;
const FAINT: u32 = 0xb4b4ba;
const DOT: u32 = 0xdcdce1;
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

pub struct BoxView {
    doc: BoxDoc,
    store: Store,
    mode: Mode,
    /// Offset of the canvas origin from the top-left of the canvas area.
    pan: Point<Pixels>,
    selected: Option<u64>,
    drag: Option<Drag>,
    viewport: gpui::Size<Pixels>,
    focus: FocusHandle,
}

impl BoxView {
    pub fn new(doc: BoxDoc, store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            doc,
            store,
            mode: Mode::Canvas,
            pan: point(px(0.), px(0.)),
            selected: None,
            drag: None,
            viewport: window.viewport_size(),
            focus,
        }
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

    fn add_paths(&mut self, paths: Vec<PathBuf>, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let (x, y) = match (self.mode, at) {
            (Mode::Canvas, Some(position)) => {
                let (x, y) = self.to_canvas(position);
                (x - ITEM_WIDTH / 2., y - CARD_HEIGHT / 2.)
            }
            // From the "+" button or the list view: drop near the middle of the visible canvas.
            _ => {
                let center = point(self.viewport.width / 2., self.viewport.height / 2.);
                let (x, y) = self.to_canvas(center);
                let row_width = paths.len().saturating_sub(1) as f32 * ambry_core::DROP_STEP_X;
                (x - ITEM_WIDTH / 2. - row_width / 2., y - CARD_HEIGHT / 2.)
            }
        };
        let added = self.doc.add_paths(&paths, x, y);
        if let Some(&last) = added.last() {
            self.selected = Some(last);
            self.save();
        }
        cx.notify();
    }

    fn pick_paths(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Add to box".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |this, cx| this.add_paths(paths, None, cx)).ok();
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
            .gap(px(10.))
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(
                div()
                    .size(px(24.))
                    .rounded(px(6.))
                    .bg(rgb(ACCENT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path("icons/frame.svg")
                            .size(px(15.))
                            .text_color(gpui::white()),
                    ),
            )
            .child(
                div()
                    .text_size(px(16.))
                    .text_color(rgb(INK))
                    .child(SharedString::from(self.doc.name.clone())),
            )
            .child(
                div()
                    .text_size(px(16.))
                    .text_color(rgb(FAINT))
                    .child(self.status()),
            )
    }

    fn render_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pan = self.pan;
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
                canvas(|_, _, _| (), move |bounds, _, window, _| paint_dots(bounds, pan, window))
                    .absolute()
                    .size_full(),
            )
            .when(self.doc.items.is_empty(), |this| this.child(empty_hint()))
            .children(
                self.doc
                    .items
                    .iter()
                    .map(|item| self.render_canvas_item(item, cx).into_any_element())
                    .collect::<Vec<_>>(),
            )
    }

    fn render_canvas_item(&self, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let id = item.id;
        let selected = self.selected == Some(id);
        let dragging = self.drag.as_ref().is_some_and(|drag| drag.id == id && drag.moved);
        div()
            .id(("item", id as usize))
            .absolute()
            .left(self.pan.x + px(item.x))
            .top(self.pan.y + px(item.y))
            .w(px(ITEM_WIDTH))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .when(!item.exists(), |this| this.opacity(0.4))
            .when(dragging, |this| this.opacity(0.85))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, window, cx| {
                    this.on_item_mouse_down(id, event, window, cx)
                }),
            )
            .child(card_face(item, selected))
            .child(
                div()
                    .max_w(px(ITEM_WIDTH))
                    .px(px(6.))
                    .rounded(px(4.))
                    .when(selected, |this| this.bg(rgb(ACCENT)))
                    .text_size(px(14.))
                    .text_color(if selected {
                        gpui::white()
                    } else {
                        rgb(QUIET).into()
                    })
                    .truncate()
                    .child(item.name()),
            )
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    this.selected = None;
                    cx.notify();
                }),
            )
            .when(self.doc.items.is_empty(), |this| this.child(empty_hint()))
            .children(self.doc.items.iter().map(|item| {
                let id = item.id;
                let selected = self.selected == Some(id);
                div()
                    .id(("row", id as usize))
                    .h(px(40.))
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
                    .child(
                        div()
                            .w(px(56.))
                            .text_size(px(12.))
                            .text_color(rgb(FAINT))
                            .child(item.badge()),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(INK))
                            .child(item.name()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_color(rgb(FAINT))
                            .truncate()
                            .child(item.path.display().to_string()),
                    )
            }))
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        let mode_button = move |id: &'static str, icon: &'static str, target: Mode| {
            let active = mode == target;
            div()
                .id(id)
                .size(px(40.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .when(active, |this| this.bg(rgb(PRESSED)))
                .child(svg().path(icon).size(px(22.)).text_color(if active {
                    rgb(INK)
                } else {
                    rgb(QUIET)
                }))
        };

        div()
            .absolute()
            .left(px(20.))
            .bottom(px(20.))
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                floating()
                    .id("avatar")
                    .size(px(52.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(18.))
                    .text_color(rgb(INK))
                    .child(user_initial()),
            )
            .child(
                floating()
                    .id("mode")
                    .h(px(52.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        mode_button("mode-canvas", "icons/frame.svg", Mode::Canvas).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.mode = Mode::Canvas;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        mode_button("mode-list", "icons/list.svg", Mode::List).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.mode = Mode::List;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                floating()
                    .id("add")
                    .size(px(52.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_click(cx.listener(|this, _, _, cx| this.pick_paths(cx)))
                    .child(svg().path("icons/plus.svg").size(px(22.)).text_color(rgb(INK))),
            )
    }
}

impl Render for BoxView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.viewport = window.viewport_size();
        div()
            .id("box")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(gpui::white())
            .text_color(rgb(INK))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.add_paths(paths.paths().to_vec(), Some(window.mouse_position()), cx);
            }))
            .map(|this| match self.mode {
                Mode::Canvas => this.child(self.render_canvas(cx)),
                Mode::List => this.child(self.render_list(cx)),
            })
            .child(self.render_titlebar())
            .child(self.render_toolbar(cx))
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

fn card_face(item: &Item, selected: bool) -> impl IntoElement {
    let border: Hsla = if selected {
        rgb(ACCENT).into()
    } else {
        rgb(HAIRLINE).into()
    };
    match item.kind {
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
            .bg(rgb(0xf7f7f9))
            .border_1()
            .when(selected, |this| this.border_2())
            .border_color(border)
            .shadow_sm()
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
                        .text_size(px(12.))
                        .text_color(rgb(FAINT))
                        .child(item.badge()),
                )
            }),
    }
}

fn empty_hint() -> impl IntoElement {
    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(14.))
        .text_color(rgb(FAINT))
        .child("Drop files or folders here")
}

/// A white rounded control that floats over the canvas, like the toolbar on the reference.
fn floating() -> gpui::Div {
    div()
        .rounded_full()
        .bg(gpui::white())
        .border_1()
        .border_color(rgb(HAIRLINE))
        .shadow_md()
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
