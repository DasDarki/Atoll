use atoll_core::{DownloadKind, DownloadStatus, Timestamp};
use atoll_storage::prelude::*;
use atoll_storage::{DownloadProgress, NewDownload, Storage};

fn storage() -> Storage {
    Storage::open_in_memory().expect("in-memory database")
}

fn start(storage: &Storage, name: &str, at: i64) -> atoll_core::DownloadId {
    storage
        .create_download(NewDownload {
            url: "https://example.org/file",
            path: None,
            file_name: name,
            total_bytes: 100,
            started_at: Timestamp(at),
            kind: DownloadKind::File,
        })
        .unwrap()
}

#[test]
fn downloads_are_listed_newest_first_with_their_progress() {
    let storage = storage();
    let first = start(&storage, "first.zip", 1);
    let second = start(&storage, "second.zip", 2);
    storage
        .update_download(
            first,
            DownloadProgress {
                path: Some("/tmp/first.zip"),
                file_name: "first.zip",
                received_bytes: 100,
                total_bytes: 100,
                status: DownloadStatus::Complete,
                finished_at: Some(Timestamp(5)),
            },
        )
        .unwrap();

    let downloads = storage.downloads(10).unwrap();
    assert_eq!(downloads.len(), 2);
    assert_eq!(downloads[0].id, second);
    assert_eq!(downloads[0].status, DownloadStatus::Running);
    assert_eq!(downloads[1].path.as_deref(), Some("/tmp/first.zip"));
    assert_eq!(downloads[1].status, DownloadStatus::Complete);
    assert_eq!(downloads[1].finished_at, Some(Timestamp(5)));
}

#[test]
fn a_known_path_is_kept_when_later_updates_have_none() {
    let storage = storage();
    let id = start(&storage, "a.pdf", 1);
    let update = |path: Option<&str>, status| {
        storage
            .update_download(
                id,
                DownloadProgress {
                    path,
                    file_name: "a.pdf",
                    received_bytes: 10,
                    total_bytes: 100,
                    status,
                    finished_at: None,
                },
            )
            .unwrap();
    };
    update(Some("/tmp/a.pdf"), DownloadStatus::Running);
    update(None, DownloadStatus::Canceled);
    let download = &storage.downloads(1).unwrap()[0];
    assert_eq!(download.path.as_deref(), Some("/tmp/a.pdf"));
    assert_eq!(download.status, DownloadStatus::Canceled);
}

#[test]
fn clearing_keeps_running_downloads_and_restarts_interrupt_them() {
    let storage = storage();
    let running = start(&storage, "running.iso", 1);
    let finished = start(&storage, "finished.iso", 2);
    storage
        .update_download(
            finished,
            DownloadProgress {
                path: None,
                file_name: "finished.iso",
                received_bytes: 100,
                total_bytes: 100,
                status: DownloadStatus::Complete,
                finished_at: Some(Timestamp(3)),
            },
        )
        .unwrap();

    storage.clear_finished_downloads().unwrap();
    let remaining = storage.downloads(10).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, running);

    assert_eq!(
        storage.interrupt_running_downloads(Timestamp(9)).unwrap(),
        1
    );
    let interrupted = &storage.downloads(1).unwrap()[0];
    assert_eq!(interrupted.status, DownloadStatus::Interrupted);
    assert_eq!(interrupted.finished_at, Some(Timestamp(9)));

    storage.remove_download(running).unwrap();
    assert!(storage.downloads(10).unwrap().is_empty());
}

#[test]
fn video_downloads_keep_their_kind() {
    let storage = storage();
    let file = start(&storage, "file.zip", 1);
    let video = storage
        .create_download(NewDownload {
            url: "https://videos.example/watch?v=1",
            path: None,
            file_name: "Katze.mp4",
            total_bytes: 0,
            started_at: Timestamp(2),
            kind: DownloadKind::Video,
        })
        .unwrap();
    let downloads = storage.downloads(10).unwrap();
    assert_eq!(downloads[0].id, video);
    assert_eq!(downloads[0].kind, DownloadKind::Video);
    assert_eq!(downloads[1].id, file);
    assert_eq!(downloads[1].kind, DownloadKind::File);
}
