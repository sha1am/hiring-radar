pub mod greenhouse;
pub mod linkedin_guest;
pub mod linkedin_voyager;

use crate::model::RawPost;
use crate::settings::Settings;

/// Every source is a plug: fetch a batch of current posts. The pipeline handles
/// dedup, classification, scoring and release — sources only retrieve + normalise.
#[async_trait::async_trait]
pub trait JobSource: Send + Sync {
    fn name(&self) -> &str;
    /// Sources read their targets from the live settings snapshot rather than
    /// from construction-time config, so boards and queries can be edited in
    /// the dashboard without a restart. A source with nothing configured
    /// returns an empty batch; the crawl loop treats that as a no-op.
    async fn fetch(&self, s: &Settings) -> anyhow::Result<Vec<RawPost>>;
}
