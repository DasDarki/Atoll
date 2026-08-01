use std::cell::RefCell;

/// An image from the clipboard as RGBA pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The system clipboard. The handle is kept alive because on Linux the owning process has to
/// serve the clipboard content for as long as it is selected.
#[derive(Default)]
pub struct Clipboard {
    inner: RefCell<Option<arboard::Clipboard>>,
}

impl Clipboard {
    pub fn set_text(&self, text: &str) -> bool {
        self.with(|clipboard| clipboard.set_text(text.to_owned()).is_ok())
            .unwrap_or(false)
    }

    pub fn text(&self) -> Option<String> {
        self.with(|clipboard| clipboard.get_text().ok()).flatten()
    }

    /// The image in the clipboard, if it holds one.
    pub fn image(&self) -> Option<ClipboardImage> {
        let image = self
            .with(|clipboard| clipboard.get_image().ok())
            .flatten()?;
        let width = u32::try_from(image.width).ok()?;
        let height = u32::try_from(image.height).ok()?;
        (width > 0 && height > 0).then(|| ClipboardImage {
            width,
            height,
            rgba: image.bytes.into_owned(),
        })
    }

    fn with<T>(&self, action: impl FnOnce(&mut arboard::Clipboard) -> T) -> Option<T> {
        let mut inner = self.inner.borrow_mut();
        if inner.is_none() {
            match arboard::Clipboard::new() {
                Ok(clipboard) => *inner = Some(clipboard),
                Err(error) => {
                    tracing::warn!(%error, "clipboard unavailable");
                    return None;
                }
            }
        }
        inner.as_mut().map(action)
    }
}
