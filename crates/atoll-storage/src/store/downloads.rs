use atoll_core::{Download, DownloadId, DownloadKind, DownloadStatus, Timestamp};
use rusqlite::{Row, params};

use crate::connection::Connected;
use crate::error::StorageResult;

const COLUMNS: &str = "id, url, path, file_name, received_bytes, total_bytes, status, started_at, \
                       finished_at, kind";

/// Parameters for [`DownloadStore::create_download`].
#[derive(Debug, Clone)]
pub struct NewDownload<'a> {
    pub url: &'a str,
    pub path: Option<&'a str>,
    pub file_name: &'a str,
    pub total_bytes: i64,
    pub started_at: Timestamp,
    pub kind: DownloadKind,
}

/// The latest known state of a download, see [`DownloadStore::update_download`].
#[derive(Debug, Clone)]
pub struct DownloadProgress<'a> {
    pub path: Option<&'a str>,
    pub file_name: &'a str,
    pub received_bytes: i64,
    pub total_bytes: i64,
    pub status: DownloadStatus,
    pub finished_at: Option<Timestamp>,
}

/// The download list.
pub trait DownloadStore: Connected {
    /// Adds a running download.
    fn create_download(&self, download: NewDownload<'_>) -> StorageResult<DownloadId> {
        self.connection()
            .prepare_cached(
                "INSERT INTO download (url, path, file_name, total_bytes, status, started_at, kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?
            .execute(params![
                download.url,
                download.path,
                download.file_name,
                download.total_bytes,
                DownloadStatus::Running.as_str(),
                download.started_at.0,
                download.kind.as_str()
            ])?;
        Ok(DownloadId(self.connection().last_insert_rowid()))
    }

    fn update_download(&self, id: DownloadId, progress: DownloadProgress<'_>) -> StorageResult<()> {
        self.connection()
            .prepare_cached(
                "UPDATE download SET path = COALESCE(?1, path), file_name = ?2, received_bytes = ?3,
                 total_bytes = ?4, status = ?5, finished_at = ?6 WHERE id = ?7",
            )?
            .execute(params![
                progress.path,
                progress.file_name,
                progress.received_bytes,
                progress.total_bytes,
                progress.status.as_str(),
                progress.finished_at.map(|at| at.0),
                id.0
            ])?;
        Ok(())
    }

    /// Gives a download the file it ended up as, after it was turned into another format.
    fn rename_download(&self, id: DownloadId, file_name: &str, path: &str) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE download SET file_name = ?1, path = ?2 WHERE id = ?3")?
            .execute(params![file_name, path, id.0])?;
        Ok(())
    }

    /// The most recent downloads, newest first.
    fn downloads(&self, limit: usize) -> StorageResult<Vec<Download>> {
        let mut statement = self.connection().prepare_cached(&format!(
            "SELECT {COLUMNS} FROM download ORDER BY started_at DESC, id DESC LIMIT ?1"
        ))?;
        let downloads = statement
            .query_map([limit as i64], download)?
            .collect::<Result<_, _>>()?;
        Ok(downloads)
    }

    fn remove_download(&self, id: DownloadId) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM download WHERE id = ?1")?
            .execute([id.0])?;
        Ok(())
    }

    /// Removes every download that is no longer running.
    fn clear_finished_downloads(&self) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM download WHERE status != ?1")?
            .execute([DownloadStatus::Running.as_str()])?;
        Ok(())
    }

    /// Marks downloads that were running when Atoll last closed as interrupted.
    fn interrupt_running_downloads(&self, at: Timestamp) -> StorageResult<usize> {
        let changed = self
            .connection()
            .prepare_cached("UPDATE download SET status = ?1, finished_at = ?2 WHERE status = ?3")?
            .execute(params![
                DownloadStatus::Interrupted.as_str(),
                at.0,
                DownloadStatus::Running.as_str()
            ])?;
        Ok(changed)
    }
}

impl<T: Connected + ?Sized> DownloadStore for T {}

fn download(row: &Row<'_>) -> rusqlite::Result<Download> {
    let status: String = row.get(6)?;
    let kind: String = row.get(9)?;
    Ok(Download {
        id: DownloadId(row.get(0)?),
        url: row.get(1)?,
        path: row.get(2)?,
        file_name: row.get(3)?,
        received_bytes: row.get(4)?,
        total_bytes: row.get(5)?,
        status: DownloadStatus::parse(&status).unwrap_or(DownloadStatus::Interrupted),
        started_at: Timestamp(row.get(7)?),
        finished_at: row.get::<_, Option<i64>>(8)?.map(Timestamp),
        kind: DownloadKind::parse(&kind).unwrap_or_default(),
    })
}
