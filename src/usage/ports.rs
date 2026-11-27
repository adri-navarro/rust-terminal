//! Hexagonal port for pluggable AI usage providers.

use super::domain::ScanCollector;
use super::service::ScanCache;
use std::path::Path;

pub trait UsageProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn scan(
        &self,
        home: &Path,
        allow_network: bool,
        out: &mut ScanCollector,
        cache: &mut ScanCache,
        now: u64,
    );
}
