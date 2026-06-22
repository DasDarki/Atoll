use std::cell::RefCell;

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
