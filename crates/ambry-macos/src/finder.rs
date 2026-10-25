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
    image_png(&icon(path), points)
}

/// Finder's open folder, which a folder turns into while something is dragged over it, as a
/// PNG `points` wide for a 2x display.
pub fn open_folder_png(points: f64) -> Option<Vec<u8>> {
    // The HFS type of the open folder icon, 'ofld', as AppKit spells file types; it has no
    // content type to ask for instead.
    #[allow(deprecated)]
    let image = NSWorkspace::sharedWorkspace().iconForFileType(&NSString::from_str("'ofld'"));
    image_png(&image, points)
}

fn image_png(image: &NSImage, points: f64) -> Option<Vec<u8>> {
    let pixels = points * 2.;
    let mut rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(pixels, pixels));
    let cg_image = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(png.to_vec())
}

/// Writes the picture Finder's icon view shows for `path` to `into` as a PNG `points` wide
/// for a 2x display: Quick Look's thumbnail in icon style, or the icon with the documents
/// peeking out of a folder that has some, which `icon` never shows. Waits for Quick Look a
/// moment; false when it has nothing by then.
pub fn thumbnail_png(path: &Path, points: f64, into: &Path) -> bool {
    thumbnail_bytes(path, points).is_some_and(|png| std::fs::write(into, png).is_ok())
}

/// Quick Look's picture for `path`, as [`thumbnail_png`] makes it.
pub(crate) fn thumbnail_image(path: &Path, points: f64) -> Option<Retained<NSImage>> {
    let png = thumbnail_bytes(path, points)?;
    let data = objc2_foundation::NSData::with_bytes(&png);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(points, points));
    Some(image)
}

fn thumbnail_bytes(path: &Path, points: f64) -> Option<Vec<u8>> {
    use objc2_quick_look_thumbnailing::{
        QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes,
        QLThumbnailGenerator, QLThumbnailRepresentation,
    };

    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let request = unsafe {
        QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
            QLThumbnailGenerationRequest::alloc(),
            &url,
            NSSize::new(points, points),
            2.,
            QLThumbnailGenerationRequestRepresentationTypes::All,
        )
    };
    unsafe { request.setIconMode(true) };
    // Quick Look answers on a queue of its own; the PNG is made there and handed back.
    let (done, finished) = std::sync::mpsc::channel();
    let handler = block2::RcBlock::new(
        move |picture: *mut QLThumbnailRepresentation, _error: *mut objc2_foundation::NSError| {
            let png = unsafe { picture.as_ref() }.and_then(|picture| {
                let image = unsafe { picture.CGImage() };
                let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &image);
                unsafe {
                    rep.representationUsingType_properties(
                        NSBitmapImageFileType::PNG,
                        &NSDictionary::new(),
                    )
                }
                .map(|png| png.to_vec())
            });
            done.send(png).ok();
        },
    );
    let generator = unsafe { QLThumbnailGenerator::sharedGenerator() };
    unsafe { generator.generateBestRepresentationForRequest_completionHandler(&request, &handler) };
    match finished.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(png) => png,
        Err(_) => {
            unsafe { generator.cancelRequest(&request) };
            None
        }
    }
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
