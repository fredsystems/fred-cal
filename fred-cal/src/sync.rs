// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::cache::CacheManager;
use crate::models::CalendarData;
use anyhow::Result;
use chrono::Utc;
use fast_dav_rs::CalDavClient;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, interval};

/// Manages synchronization with `CalDAV` server
pub struct SyncManager {
    client: Arc<CalDavClient>,
    cache: Arc<CacheManager>,
    data: Arc<RwLock<CalendarData>>,
}

impl SyncManager {
    /// Create a new sync manager
    pub fn new(client: CalDavClient, cache: CacheManager) -> Result<Self> {
        let data = cache.load()?.map_or_else(
            || {
                info!("No cache found, starting fresh");
                Arc::new(RwLock::new(CalendarData::new()))
            },
            |cached_data| {
                info!("Loaded existing cache");
                Arc::new(RwLock::new(cached_data))
            },
        );

        Ok(Self {
            client: Arc::new(client),
            cache: Arc::new(cache),
            data,
        })
    }

    /// Get a read-only reference to the calendar data
    pub fn data(&self) -> Arc<RwLock<CalendarData>> {
        Arc::clone(&self.data)
    }

    /// Perform a full sync with the `CalDAV` server
    pub async fn sync(&self) -> Result<()> {
        info!("Starting calendar sync");

        let principal = self
            .client
            .discover_current_user_principal()
            .await?
            .ok_or_else(|| anyhow::anyhow!("No principal returned"))?;

        let homes = self.client.discover_calendar_home_set(&principal).await?;
        let home = homes
            .first()
            .ok_or_else(|| anyhow::anyhow!("Missing calendar-home-set"))?;

        let calendars = self.client.list_calendars(home).await?;
        debug!("Found {} calendars", calendars.len());

        // For now, we'll create placeholder data based on calendar names
        // TODO: Implement actual iCal parsing when we can fetch calendar objects
        let all_events = Vec::new();
        let all_todos = Vec::new();

        for calendar in &calendars {
            let calendar_name = calendar
                .displayname
                .clone()
                .unwrap_or_else(|| "Unnamed".to_string());
            let _calendar_url = calendar.href.clone();

            debug!("Found calendar: {}", calendar_name);

            // TODO: Fetch actual calendar objects and parse them
            // For now, this is a stub that creates the structure without real data
            // The actual implementation will need to:
            // 1. Fetch calendar objects from the server
            // 2. Parse iCalendar data (VEVENT and VTODO components)
            // 3. Extract event and todo information
            // 4. Store ETags for efficient syncing
        }

        // Update the in-memory data
        let (event_count, todo_count) = {
            let mut data = self.data.write().await;
            data.events = all_events;
            data.todos = all_todos;
            data.last_sync = Utc::now();

            let counts = (data.events.len(), data.todos.len());

            // Save to cache
            self.cache.save(&data)?;
            drop(data);

            counts
        }; // Write lock dropped here

        info!(
            "Sync complete: {} events, {} todos (from {} calendars)",
            event_count,
            todo_count,
            calendars.len()
        );

        Ok(())
    }

    /// Start a background sync task that runs periodically
    pub async fn start_periodic_sync(self: Arc<Self>, interval_minutes: u64) {
        let mut ticker = interval(Duration::from_secs(interval_minutes * 60));

        loop {
            ticker.tick().await;
            info!("Running periodic sync");

            if let Err(e) = self.sync().await {
                error!("Periodic sync failed: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_sync_manager_placeholder() {
        // Full integration tests for sync manager are in the integration test suite
        // This placeholder exists to satisfy the module structure
    }
}
