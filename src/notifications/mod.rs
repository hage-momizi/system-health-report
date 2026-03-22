use anyhow::Result;
use async_trait::async_trait;

use crate::report::Report;

pub mod bot;
pub mod webhook;

#[async_trait]
pub trait Notifier: Send + Sync {
    async fn send_report(&self, report: &Report) -> Result<()>;
}
