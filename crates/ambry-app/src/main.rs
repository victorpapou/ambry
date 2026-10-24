mod assets;
mod box_view;
#[cfg(target_os = "macos")]
mod native_view;

use ambry_core::{BoxDoc, Store};
use gpui::{
    App, AppContext as _, Application, Bounds, KeyBinding, Menu, MenuItem, OsAction,
    TitlebarOptions, WindowBounds, WindowOptions, actions, px, size,
};

use crate::box_view::BoxView;

const DEFAULT_BOX: &str = "Inbox";

actions!(
    ambry,
    [
        OpenSettings,
        Hide,
        HideOthers,
        Quit,
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        SelectAll
    ]
);

/// The menu bar: the app menu with Settings, and the Edit menu, whose commands reach the text
/// fields of native windows such as Settings.
fn set_up_menus(cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &OpenSettings, _| {
        ambry_macos::settings::open(ambry_macos::settings::Pane::General)
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.bind_keys([
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("shift-cmd-z", Redo, None),
        KeyBinding::new("cmd-x", Cut, None),
        KeyBinding::new("cmd-c", Copy, None),
        KeyBinding::new("cmd-v", Paste, None),
        KeyBinding::new("cmd-a", SelectAll, None),
    ]);
    cx.set_menus(vec![
        Menu {
            name: "Ambry".into(),
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Hide Ambry", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::separator(),
                MenuItem::action("Quit Ambry", Quit),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", Undo, OsAction::Undo),
                MenuItem::os_action("Redo", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", Cut, OsAction::Cut),
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            ],
        },
    ]);
}

fn main() {
    Application::new()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            #[cfg(target_os = "macos")]
            ambry_macos::settings::apply_appearance();
            set_up_menus(cx);
            let store = Store::in_data_dir("inbox").expect("no data directory");
            let doc = store.load_or_new(DEFAULT_BOX).unwrap_or_else(|err| {
                eprintln!("could not load {}: {err:#}", store.path().display());
                BoxDoc::new(DEFAULT_BOX)
            });

            let bounds = Bounds::centered(None, size(px(720.), px(520.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(doc.name.clone().into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    window_min_size: Some(size(px(480.), px(360.))),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| BoxView::new(doc, store, window, cx)),
            )
            .expect("failed to open window");

            #[cfg(target_os = "macos")]
            if std::env::args().any(|arg| arg == "--lab") {
                ambry_macos::open_switch_lab();
            }

            cx.on_window_closed(|cx| cx.quit()).detach();
            cx.activate(true);
        });
}
