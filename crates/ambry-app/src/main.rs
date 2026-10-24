mod assets;
mod box_view;
#[cfg(target_os = "macos")]
mod native_view;

use ambry_core::{BoxDoc, Store};
use gpui::{
    App, AppContext as _, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point,
    px, size,
};

use crate::box_view::BoxView;

const DEFAULT_BOX: &str = "Inbox";

fn main() {
    Application::new()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
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
                        // Centred on the title row, which GPUI draws.
                        traffic_light_position: Some(point(px(13.), px(12.))),
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
