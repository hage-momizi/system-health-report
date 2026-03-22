use anyhow::Result;
use chrono::{NaiveTime, Timelike, Utc};
use std::future::Future;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};

use crate::config::Config;

pub async fn run_scheduler<F, Fut>(config: Arc<Config>, run_fn: F) -> Result<()>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Result<()>> + Send,
{
    let schedule = &config.ai_assistant.schedule;

    if !schedule.enabled {
        info!("Scheduler is disabled in config");
        return Ok(());
    }

    if schedule.times.is_empty() {
        warn!("No scheduled times configured");
        return Ok(());
    }

    info!(
        "Starting scheduler with times: {:?} (timezone: {})",
        schedule.times, schedule.timezone
    );

    // Parse scheduled times
    let mut scheduled_times: Vec<NaiveTime> = Vec::new();
    for time_str in &schedule.times {
        match NaiveTime::parse_from_str(time_str, "%H:%M") {
            Ok(t) => scheduled_times.push(t),
            Err(e) => {
                error!("Failed to parse scheduled time '{}': {}", time_str, e);
            }
        }
    }

    if scheduled_times.is_empty() {
        return Err(anyhow::anyhow!("No valid scheduled times found"));
    }

    scheduled_times.sort();

    loop {
        let now = Utc::now();
        let current_time = NaiveTime::from_hms_opt(now.hour(), now.minute(), now.second())
            .unwrap_or_else(|| NaiveTime::from_hms_opt(0, 0, 0).unwrap());

        // Find the next scheduled time
        let next_time = scheduled_times
            .iter()
            .find(|&&t| t > current_time)
            .copied();

        let (next_time, add_day) = match next_time {
            Some(t) => (t, false),
            None => {
                // All times today have passed, use first time tomorrow
                (scheduled_times[0], true)
            }
        };

        // Calculate seconds until next run
        let secs_until_next = {
            let next_secs = next_time.num_seconds_from_midnight() as i64;
            let current_secs = current_time.num_seconds_from_midnight() as i64;

            if add_day {
                86400 - current_secs + next_secs
            } else {
                next_secs - current_secs
            }
        };

        let secs_to_wait = if secs_until_next <= 0 {
            60
        } else {
            secs_until_next as u64
        };

        info!(
            "Next scheduled run in {} seconds (at {:02}:{:02} UTC)",
            secs_to_wait,
            next_time.hour(),
            next_time.minute()
        );

        sleep(Duration::from_secs(secs_to_wait)).await;

        info!("Running scheduled health report");
        if let Err(e) = run_fn().await {
            error!("Scheduled run failed: {}", e);
        }

        // Small delay to avoid immediate re-run
        sleep(Duration::from_secs(60)).await;
    }
}
