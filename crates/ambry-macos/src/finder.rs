//! What Finder shows for a file: its icon, kind, size and modification date.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSWorkspace};
use objc2_foundation::{
    NSByteCountFormatter, NSByteCountFormatterCountStyle, NSDate, NSDateFormatter,
    NSDateFormatterStyle, NSDictionary, NSPoint, NSRect, NSSize, NSString, NSURL,
    NSURLLocalizedTypeDescriptionKey,
};

/// Finder's icon for `path`.
pub(crate) fn icon(path: &Path) -> Retained<NSImage> {
    NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(&path.to_string_lossy()))
}

/// Finder's icon for `path` as a PNG `points` wide, rendered for a 2x display.
pub fn icon_png(path: &Path, points: f64) -> Option<Vec<u8>> {
    let image = icon(path);
    let pixels = points * 2.;
    let mut rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(pixels, pixels));
    let cg_image = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(png.to_vec())
}

/// Finder's "Kind" for `path`, like "Markdown document" or "Folder".
pub fn kind(path: &Path) -> Option<String> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let mut value = None;
    unsafe { url.getResourceValue_forKey_error(&mut value, NSURLLocalizedTypeDescriptionKey) }
        .ok()?;
    Some(value?.downcast::<NSString>().ok()?.to_string())
}

/// A file size the way Finder writes it, like "12 KB".
pub fn size(bytes: u64) -> String {
    NSByteCountFormatter::stringFromByteCount_countStyle(
        bytes as i64,
        NSByteCountFormatterCountStyle::File,
    )
    .to_string()
}

/// A date the way Finder's list view writes it, like "Today at 13:19".
pub fn date(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0., |duration| duration.as_secs_f64());
    let formatter = NSDateFormatter::new();
    formatter.setDateStyle(NSDateFormatterStyle::MediumStyle);
    formatter.setTimeStyle(NSDateFormatterStyle::ShortStyle);
    formatter.setDoesRelativeDateFormatting(true);
    formatter
        .stringFromDate(&NSDate::dateWithTimeIntervalSince1970(seconds))
        .to_string()
}
