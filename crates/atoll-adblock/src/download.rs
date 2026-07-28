use std::time::Duration;

use thiserror::Error;

use crate::lists::{FilterList, is_filter_list};

/// The largest list accepted, current lists are a few megabytes.
const MAX_LIST_SIZE: u64 = 32 * 1024 * 1024;

const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Error)]
pub(crate) enum DownloadError {
    #[error("{list} could not be downloaded: {source}")]
    Request {
        list: &'static str,
        #[source]
        source: ureq::Error,
    },
    #[error("{list} is not a filter list")]
    NotAList { list: &'static str },
}

/// Downloads the text of `list`.
pub(crate) fn fetch(list: &FilterList) -> Result<String, DownloadError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent(concat!("Atoll/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let request_error = |source| DownloadError::Request {
        list: list.name,
        source,
    };
    let mut response = agent.get(list.url).call().map_err(request_error)?;
    let text = response
        .body_mut()
        .with_config()
        .limit(MAX_LIST_SIZE)
        .read_to_string()
        .map_err(request_error)?;
    if is_filter_list(&text) {
        Ok(text)
    } else {
        Err(DownloadError::NotAList { list: list.name })
    }
}

impl DownloadError {
    pub(crate) fn list(&self) -> &'static str {
        match self {
            DownloadError::Request { list, .. } | DownloadError::NotAList { list } => list,
        }
    }
}
