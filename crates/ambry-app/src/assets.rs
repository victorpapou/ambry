use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, SharedString};

/// Icons compiled into the binary so `cargo run` works from any directory.
pub struct Assets;

const ICONS: &[(&str, &[u8])] = &[
    (
        "icons/folder.svg",
        include_bytes!("../assets/icons/folder.svg"),
    ),
    (
        "icons/frame.svg",
        include_bytes!("../assets/icons/frame.svg"),
    ),
    ("icons/list.svg", include_bytes!("../assets/icons/list.svg")),
    ("icons/plus.svg", include_bytes!("../assets/icons/plus.svg")),
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}
