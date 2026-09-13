pub mod greenhouse;
pub mod linkedin_guest;
pub mod linkedin_voyager;

use crate::model::RawPost;

/// Every source is a plug: fetch a batch of current posts. The pipeline handles
/// dedup, classification, scoring and release — sources only retrieve + normalise.
#[async_trait::async_trait]
pub trait JobSource: Send + Sync {
    fn name(&self) -> &str;
    async fn fetch(&self) -> anyhow::Result<Vec<RawPost>>;
}
