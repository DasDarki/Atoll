use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;

/// What kind of native file dialog to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileDialogKind {
    Open,
    OpenMultiple,
    OpenFolder,
    Save,
}

#[derive(Debug, Clone)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FileDialogOptions {
    pub kind: FileDialogKind,
    pub title: String,
    pub directory: Option<PathBuf>,
    pub file_name: Option<String>,
    pub filters: Vec<FileFilter>,
}

type Completion = Box<dyn FnOnce(Option<Vec<PathBuf>>)>;

thread_local! {
    static PENDING: RefCell<HashMap<u64, Completion>> = RefCell::default();
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// Shows a native file dialog without blocking the event loop. `done` runs on the main thread
/// with the chosen paths, or `None` if the dialog was canceled.
pub fn show(options: FileDialogOptions, done: impl FnOnce(Option<Vec<PathBuf>>) + 'static) {
    let id = NEXT_ID.with(|next| {
        let id = next.get() + 1;
        next.set(id);
        id
    });
    PENDING.with_borrow_mut(|pending| pending.insert(id, Box::new(done)));

    std::thread::spawn(move || {
        let result = run(&options);
        let delivered = slint::invoke_from_event_loop(move || {
            if let Some(done) = PENDING.with_borrow_mut(|pending| pending.remove(&id)) {
                done(result);
            }
        });
        if delivered.is_err() {
            tracing::warn!("file dialog result arrived after the event loop ended");
        }
    });
}

fn run(options: &FileDialogOptions) -> Option<Vec<PathBuf>> {
    let mut dialog = rfd::FileDialog::new().set_title(&options.title);
    if let Some(directory) = &options.directory {
        dialog = dialog.set_directory(directory);
    }
    if let Some(file_name) = &options.file_name {
        dialog = dialog.set_file_name(file_name);
    }
    for filter in &options.filters {
        dialog = dialog.add_filter(&filter.name, &filter.extensions);
    }
    match options.kind {
        FileDialogKind::Open => dialog.pick_file().map(|path| vec![path]),
        FileDialogKind::OpenMultiple => dialog.pick_files(),
        FileDialogKind::OpenFolder => dialog.pick_folder().map(|path| vec![path]),
        FileDialogKind::Save => dialog.save_file().map(|path| vec![path]),
    }
}
