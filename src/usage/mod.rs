//! Bounded context: AI agent & model usage tracking.

pub mod domain;
pub mod infra;
pub mod ports;
pub mod service;

pub use domain::{
    AgentUsage, ModelUsage, TimeRange, TokenStats, UsageReport, UsageSnapshot, WindowQuota,
    estimate_cost_usd, format_cost, format_duration_short, format_tokens, parse_iso8601,
};
pub use ports::UsageProvider;
pub use service::{ScanCache, refresh_live_quotas_async, scan_local_usage, scan_with_providers};
#[cfg(test)]
pub use service::{scan_usage_from_home, scan_usage_from_home_cached};
