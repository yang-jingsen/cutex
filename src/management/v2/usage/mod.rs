mod model;
mod pricing;
mod reducer;
mod report;
mod store;

use std::path::Path;

use crate::config::paths::runtime_dir;
use crate::management::v2::model::EventEnvelope;

pub use model::ResetWindowKind;
pub use model::TokenUsageBreakdown;
pub use model::UsageAttribution;
pub use model::UsageLedger;
pub use model::UsageLedgerEntry;
pub use model::UsageResetBoundary;
pub use model::UsageSample;
pub use model::UsageStateSnapshot;
pub use model::UsageTokenSource;
pub use pricing::DerivedUsageEstimate;
pub use pricing::UsageRateCard;
pub use report::build_usage_report;
pub use report::UsagePeriodBucket;
pub use report::UsagePricingGap;
pub use report::UsageReport;
pub use report::UsageReportCoverage;
pub use report::UsageReportGroup;
pub use report::UsageReportGroupBy;
pub use report::UsageReportPeriod;
pub use report::UsageReportQuery;
pub use report::UsageReportRow;
pub use report::UsageReportScope;
pub use report::UsageReportTotals;

pub fn record_usage_event(
    envelope: &EventEnvelope,
    launched_profile: Option<&str>,
) -> anyhow::Result<()> {
    let root = runtime_dir()?.join("management-v2");
    record_usage_event_at(&root, envelope, launched_profile)
}

pub fn load_usage_state_snapshot() -> anyhow::Result<UsageStateSnapshot> {
    let root = runtime_dir()?.join("management-v2");
    store::load_usage_state_snapshot_at(&root)
}

pub fn load_usage_ledger() -> anyhow::Result<UsageLedger> {
    let root = runtime_dir()?.join("management-v2");
    store::load_usage_ledger_at(&root)
}

pub fn load_usage_data() -> anyhow::Result<(UsageStateSnapshot, UsageLedger)> {
    let root = runtime_dir()?.join("management-v2");
    store::load_usage_data_at(&root)
}

fn record_usage_event_at(
    root: &Path,
    envelope: &EventEnvelope,
    launched_profile: Option<&str>,
) -> anyhow::Result<()> {
    let Some(event) = reducer::parse_usage_event(envelope)? else {
        return Ok(());
    };
    store::update_usage_state_at(root, &envelope.received_at, |state| {
        reducer::reduce_usage_event(state, envelope, launched_profile, event)
    })
}

#[cfg(test)]
mod report_tests;
#[cfg(test)]
mod tests;
