use std::collections::HashMap;

use atoll_core::address::host_of;
use atoll_core::{FaviconId, Tab};
use atoll_storage::Storage;
use atoll_storage::prelude::*;
use slint::Image;

use crate::bridge::images::decode_png;

/// Decoded favicons, shared by all tabs of the same host.
#[derive(Default)]
pub struct FaviconCache {
    images: HashMap<FaviconId, Image>,
    hosts: HashMap<String, FaviconId>,
    hosts_loaded: bool,
}

impl FaviconCache {
    /// Makes sure every favicon needed by `tabs` is decoded.
    pub fn prepare<'a>(&mut self, storage: &Storage, tabs: impl Iterator<Item = &'a Tab>) {
        if !self.hosts_loaded {
            match storage.favicon_hosts() {
                Ok(hosts) => {
                    self.hosts = hosts.into_iter().map(|(id, host)| (host, id)).collect();
                    self.hosts_loaded = true;
                }
                Err(error) => tracing::warn!(%error, "favicon hosts could not be loaded"),
            }
        }
        let mut missing: Vec<FaviconId> = tabs
            .filter_map(|tab| self.favicon_id(tab))
            .filter(|id| !self.images.contains_key(id))
            .collect();
        missing.sort_unstable();
        missing.dedup();
        if missing.is_empty() {
            return;
        }
        match storage.favicons(&missing) {
            Ok(favicons) => {
                for (id, png) in favicons {
                    if let Some(image) = decode_png(&png) {
                        self.images.insert(id, image);
                    }
                }
            }
            Err(error) => tracing::warn!(%error, "favicons could not be loaded"),
        }
    }

    pub fn image(&self, tab: &Tab) -> Option<Image> {
        self.favicon_id(tab)
            .and_then(|id| self.images.get(&id).cloned())
    }

    pub fn insert(&mut self, id: FaviconId, host: String, png: &[u8]) {
        if let Some(image) = decode_png(png) {
            self.images.insert(id, image);
        }
        self.hosts.insert(host, id);
    }

    fn favicon_id(&self, tab: &Tab) -> Option<FaviconId> {
        tab.favicon_id
            .or_else(|| host_of(&tab.url).and_then(|host| self.hosts.get(&host).copied()))
    }
}
