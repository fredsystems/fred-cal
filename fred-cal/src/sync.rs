// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::cache::CacheManager;
use crate::models::{CalendarData, CalendarEvent, Todo};
use crate::recurrence::{RecurrenceConfig, expand_recurring_event};
use anyhow::Result;
use chrono::{DateTime, Local, TimeZone, Utc};
use chrono_tz::Tz;
use fast_dav_rs::CalDavClient;
use futures::future::join_all;
use icalendar::{
    Calendar, CalendarDateTime, Component, DatePerhapsTime, Event, EventLike, Todo as IcalTodo,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, interval};

/// Batch size for calendar-multiget requests
const BATCH_SIZE: usize = 500;

/// Manages synchronization with `CalDAV` server
pub struct SyncManager {
    client: Arc<CalDavClient>,
    cache: Arc<CacheManager>,
    data: Arc<RwLock<CalendarData>>,
    calendar_colors: Arc<RwLock<std::collections::HashMap<String, String>>>,
}

impl SyncManager {
    /// Create a new sync manager
    ///
    /// # Errors
    ///
    /// Returns an error if the cache cannot be loaded from disk.
    pub fn new(client: CalDavClient, cache: CacheManager) -> Result<Self> {
        let data = cache.load()?.map_or_else(
            || {
                info!("No cache found, starting fresh");
                Arc::new(RwLock::new(CalendarData::new()))
            },
            |cached_data| {
                info!(
                    "Loaded existing cache: {} events, {} todos, {} calendars with sync tokens",
                    cached_data.events.len(),
                    cached_data.todos.len(),
                    cached_data.sync_tokens.len()
                );
                for (url, token) in &cached_data.sync_tokens {
                    debug!("  Calendar {}: token = {}", url, token);
                }
                Arc::new(RwLock::new(cached_data))
            },
        );

        Ok(Self {
            client: Arc::new(client),
            cache: Arc::new(cache),
            data,
            calendar_colors: Arc::new(RwLock::new(std::collections::HashMap::new())),
        })
    }

    /// Get a read-only reference to the calendar data
    #[must_use]
    pub fn data(&self) -> Arc<RwLock<CalendarData>> {
        Arc::clone(&self.data)
    }

    /// Perform a full sync with the `CalDAV` server
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The `CalDAV` server cannot be reached
    /// - Authentication fails
    /// - The server returns invalid data
    /// - The cache cannot be saved to disk
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

        // Log sync tokens from calendar list (if provided by server)
        for calendar in &calendars {
            let name = calendar
                .displayname
                .as_ref()
                .map_or("unnamed", |s| s.as_str());
            debug!(
                "Calendar '{}' has sync_token from list_calendars: {:?}",
                name,
                calendar.sync_token.as_ref().map(|t| if t.len() > 50 {
                    format!("{}... ({} chars)", &t[..50], t.len())
                } else {
                    t.clone()
                })
            );
        }

        // Check if server supports WebDAV sync
        let supports_sync = self.client.supports_webdav_sync().await.unwrap_or(false);
        if supports_sync {
            info!("Server supports WebDAV sync - using incremental updates");
        } else {
            info!("Server does not support WebDAV sync - using full sync");
        }

        // Track calendar URLs we see during this sync
        let mut active_calendar_urls = std::collections::HashSet::new();

        // Store calendar colors first (quick, can be done sequentially)
        for calendar in &calendars {
            let calendar_url = &calendar.href;
            active_calendar_urls.insert(calendar_url.clone());

            if let Some(color) = &calendar.color {
                self.calendar_colors
                    .write()
                    .await
                    .insert(calendar_url.clone(), color.clone());
            }
        }

        // Process all calendars concurrently
        let sync_tasks: Vec<_> = calendars
            .iter()
            .map(|calendar| self.sync_single_calendar(calendar, supports_sync))
            .collect();

        // Execute all calendar syncs concurrently
        join_all(sync_tasks).await;

        // Clean up events/todos from calendars that no longer exist
        let (removed_events, removed_todos) = {
            let mut data = self.data.write().await;

            let initial_events = data.events.len();
            let initial_todos = data.todos.len();

            // Remove events from calendars not in the active list
            data.events
                .retain(|e| active_calendar_urls.contains(&e.calendar_url));

            // Remove todos from calendars not in the active list
            data.todos
                .retain(|t| active_calendar_urls.contains(&t.calendar_url));

            // Remove sync tokens from calendars not in the active list
            data.sync_tokens
                .retain(|url, _| active_calendar_urls.contains(url));

            let removed = (
                initial_events - data.events.len(),
                initial_todos - data.todos.len(),
            );

            drop(data);
            removed
        };

        if removed_events > 0 || removed_todos > 0 {
            info!(
                "Cleaned up {} events and {} todos from deleted calendars",
                removed_events, removed_todos
            );
        }

        // Update last sync time and save cache
        let (event_count, todo_count) = {
            let mut data = self.data.write().await;
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

    /// Sync a single calendar with appropriate strategy
    async fn sync_single_calendar(
        &self,
        calendar: &fast_dav_rs::CalendarInfo,
        supports_sync: bool,
    ) {
        let calendar_name = calendar
            .displayname
            .clone()
            .unwrap_or_else(|| "Unnamed".to_string());
        let calendar_url = calendar.href.clone();

        debug!("Syncing calendar: {}", calendar_name);

        // Choose sync strategy
        let sync_result = if supports_sync {
            debug!(
                "Using sync_collection for {} (subsequent sync)",
                calendar_name
            );
            match self
                .sync_calendar_incremental(&calendar_url, &calendar_name)
                .await
            {
                Ok(()) => Ok(()),
                Err(e) => {
                    warn!(
                        "Incremental sync failed for {}, falling back to time-range: {}",
                        calendar_name, e
                    );
                    self.sync_calendar_full(&calendar_url, &calendar_name).await
                }
            }
        } else {
            debug!(
                "Using time-range query for {} (faster than full sync)",
                calendar_name
            );
            self.sync_calendar_full(&calendar_url, &calendar_name).await
        };

        if let Err(e) = sync_result {
            error!(
                "Failed to sync calendar {} at {}: {:?}",
                calendar_name, calendar_url, e
            );
        }
    }

    /// Perform incremental sync using `WebDAV` sync-collection
    ///
    /// # Errors
    ///
    /// Returns an error if the sync-collection query fails or sync token is invalid.
    async fn sync_calendar_incremental(
        &self,
        calendar_url: &str,
        calendar_name: &str,
    ) -> Result<()> {
        // Get sync token from cache
        let sync_token = {
            let data = self.data.read().await;
            data.sync_tokens.get(calendar_url).cloned()
        };

        // Convert empty string to None (empty string is just a marker that calendar has been synced)
        // If token is "NO_SYNC", this calendar should never use incremental sync
        let sync_token = sync_token.and_then(|t| {
            if t.is_empty() {
                None
            } else if t == "NO_SYNC" {
                // This shouldn't happen - NO_SYNC should prevent us from getting here
                None
            } else {
                Some(t)
            }
        });

        info!(
            "Incremental sync for {} with token: {}",
            calendar_name,
            sync_token
                .as_ref()
                .map_or("none (first incremental sync)", |t| t)
        );

        // Perform sync-collection query
        // Pass false for include_data - iCloud doesn't return data in sync-collection
        // We'll fetch the data separately for changed items
        // Set a limit - some servers (like iCloud) may require this to return sync tokens
        let sync_response = self
            .client
            .sync_collection(calendar_url, sync_token.as_deref(), Some(1000), false)
            .await?;

        debug!(
            "sync_collection returned {} items for {}",
            sync_response.items.len(),
            calendar_name
        );
        debug!(
            "sync_collection response: sync_token={:?}, items_count={}",
            sync_response.sync_token,
            sync_response.items.len()
        );

        let mut added_events = 0;
        let mut added_todos = 0;
        let mut deleted_count = 0;

        // Separate deleted items from changed items
        let mut hrefs_to_fetch = Vec::new();

        for item in &sync_response.items {
            if item.is_deleted {
                deleted_count += self.process_deleted_item(&item.href).await;
            } else if !item.href.ends_with('/') {
                // Skip calendar collections, collect .ics files to fetch
                hrefs_to_fetch.push(item.href.clone());
            }
        }

        if !hrefs_to_fetch.is_empty() {
            let (events, todos) = self
                .batch_fetch_calendar_items(calendar_url, calendar_name, &hrefs_to_fetch)
                .await;
            added_events += events;
            added_todos += todos;
        }

        // Store new sync token
        if let Some(new_token) = &sync_response.sync_token {
            // Check if token changed before logging
            let token_changed = {
                let data = self.data.read().await;
                data.sync_tokens.get(calendar_url) != Some(new_token)
            };

            if token_changed {
                info!(
                    "Storing new sync token for {}: {}",
                    calendar_name, new_token
                );
            }

            let mut data = self.data.write().await;
            data.sync_tokens
                .insert(calendar_url.to_string(), new_token.clone());
            drop(data);
        } else {
            warn!(
                "No sync token returned for {} - server doesn't support sync tokens, marking to never use incremental sync",
                calendar_name
            );
            // Store special marker "NO_SYNC" to prevent trying incremental sync again
            let mut data = self.data.write().await;
            data.sync_tokens
                .insert(calendar_url.to_string(), "NO_SYNC".to_string());
            drop(data);
        }

        info!(
            "Incremental sync for {}: +{} events, +{} todos, -{} deleted",
            calendar_name, added_events, added_todos, deleted_count
        );

        Ok(())
    }

    /// Batch fetch calendar items using calendar-multiget
    ///
    /// Returns (`events_count`, `todos_count`)
    async fn batch_fetch_calendar_items(
        &self,
        calendar_url: &str,
        calendar_name: &str,
        hrefs: &[String],
    ) -> (usize, usize) {
        info!(
            "Fetching {} changed items for {} in batches",
            hrefs.len(),
            calendar_name
        );

        let mut added_events = 0;
        let mut added_todos = 0;

        // Batch fetch items using calendar-multiget
        for (batch_num, chunk) in hrefs.chunks(BATCH_SIZE).enumerate() {
            debug!(
                "Fetching batch {}/{} ({} items) for {}",
                batch_num + 1,
                hrefs.len().div_ceil(BATCH_SIZE),
                chunk.len(),
                calendar_name
            );

            match self
                .client
                .calendar_multiget(calendar_url, chunk, true)
                .await
            {
                Ok(objects) => {
                    for obj in objects {
                        if let Some(ical_data) = obj.calendar_data {
                            let (events, todos) = self
                                .process_calendar_item(
                                    &ical_data,
                                    &obj.href,
                                    obj.etag.as_ref(),
                                    calendar_name,
                                    calendar_url,
                                )
                                .await;
                            added_events += events;
                            added_todos += todos;
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to fetch batch {} for {}: {}",
                        batch_num + 1,
                        calendar_name,
                        e
                    );
                }
            }
        }

        (added_events, added_todos)
    }

    /// Perform full sync of a calendar
    ///
    /// # Errors
    ///
    /// Returns an error if the calendar query fails or the `CalDAV` server is unreachable.
    async fn sync_calendar_full(&self, calendar_url: &str, calendar_name: &str) -> Result<()> {
        debug!("Full sync for {}", calendar_name);

        // Fetch all calendar objects
        let (events, todos) = self
            .fetch_and_parse_calendar(calendar_url, calendar_name)
            .await?;

        debug!(
            "Full sync for {}: fetched {} events and {} todos",
            calendar_name,
            events.len(),
            todos.len()
        );

        // Replace all items for this calendar
        let mut data = self.data.write().await;

        // Remove old items from this calendar
        data.events.retain(|e| e.calendar_url != calendar_url);
        data.todos.retain(|t| t.calendar_url != calendar_url);

        // Add new items
        data.events.extend(events);
        data.todos.extend(todos);

        drop(data);

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

    /// Process a deleted item by removing it from events and todos
    async fn process_deleted_item(&self, href: &str) -> usize {
        let mut data = self.data.write().await;

        // Remove from events
        let initial_events = data.events.len();
        data.events
            .retain(|e| !href.ends_with(&format!("{}.ics", e.uid)));
        let events_deleted = initial_events - data.events.len();

        // Remove from todos
        let initial_todos = data.todos.len();
        data.todos
            .retain(|t| !href.ends_with(&format!("{}.ics", t.uid)));
        let todos_deleted = initial_todos - data.todos.len();

        debug!("Deleted item: {}", href);
        drop(data);

        events_deleted + todos_deleted
    }

    /// Process a calendar item (parse and add/update events and todos)
    async fn process_calendar_item(
        &self,
        ical_data: &str,
        href: &str,
        etag: Option<&String>,
        calendar_name: &str,
        calendar_url: &str,
    ) -> (usize, usize) {
        debug!("Parsing iCalendar data for {}", href);

        match ical_data.parse::<Calendar>() {
            Ok(calendar) => {
                let mut data = self.data.write().await;
                let mut events_added = 0;
                let mut todos_added = 0;

                // Process events
                for event_comp in calendar.events() {
                    // Get calendar color
                    let calendar_color = {
                        let colors = self.calendar_colors.read().await;
                        colors.get(calendar_url).cloned()
                    };

                    match parse_event(
                        event_comp,
                        calendar_name,
                        calendar_url,
                        calendar_color.as_deref(),
                        etag.map(String::as_str),
                    ) {
                        Ok(event) => {
                            // Remove old instances of this event (by UID)
                            data.events.retain(|e| e.uid != event.uid);

                            // Expand recurring events
                            let config = RecurrenceConfig::default();

                            let instances = expand_recurring_event(&event, &config);

                            let instance_count = instances.len();
                            for instance in instances {
                                data.events.push(instance);
                            }

                            events_added += instance_count;
                        }
                        Err(e) => warn!("Failed to parse event: {}", e),
                    }
                }

                // Process todos
                for todo_comp in calendar.todos() {
                    match parse_todo(
                        todo_comp,
                        calendar_name,
                        calendar_url,
                        etag.map(String::as_str),
                    ) {
                        Ok(todo) => {
                            data.todos.retain(|t| t.uid != todo.uid);
                            data.todos.push(todo);
                            todos_added += 1;
                        }
                        Err(e) => warn!("Failed to parse todo: {}", e),
                    }
                }

                drop(data);
                (events_added, todos_added)
            }
            Err(e) => {
                warn!("Failed to parse iCalendar data from {}: {}", href, e);
                debug!("iCalendar data that failed to parse: {}", ical_data);
                (0, 0)
            }
        }
    }

    /// Fetch calendar objects and parse them into events and todos
    ///
    /// # Errors
    ///
    /// Returns an error if the calendar query fails or the `CalDAV` server is unreachable.
    async fn fetch_and_parse_calendar(
        &self,
        calendar_url: &str,
        calendar_name: &str,
    ) -> Result<(Vec<CalendarEvent>, Vec<Todo>)> {
        let mut events = Vec::new();
        let mut todos = Vec::new();

        // Fetch VEVENTs (calendar events)
        debug!("Querying VEVENTs from: {}", calendar_url);
        match self
            .client
            .calendar_query_timerange(calendar_url, "VEVENT", None, None, true)
            .await
        {
            Ok(objects) => {
                debug!("Fetched {} VEVENTs from {}", objects.len(), calendar_name);
                for obj in objects {
                    if let Some(ical_data) = obj.calendar_data {
                        let etag = obj.etag.clone();
                        match ical_data.parse::<Calendar>() {
                            Ok(calendar) => {
                                for event_comp in calendar.events() {
                                    // Get calendar color
                                    let calendar_color = {
                                        let colors = self.calendar_colors.read().await;
                                        colors.get(calendar_url).cloned()
                                    };

                                    match parse_event(
                                        event_comp,
                                        calendar_name,
                                        calendar_url,
                                        calendar_color.as_deref(),
                                        etag.as_deref(),
                                    ) {
                                        Ok(event) => {
                                            // Expand recurring events
                                            let config = RecurrenceConfig::default();
                                            let instances = expand_recurring_event(&event, &config);
                                            for instance in instances {
                                                events.push(instance);
                                            }
                                        }
                                        Err(e) => {
                                            warn!("Failed to parse event: {}", e);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Failed to parse iCalendar data from {}: {}", obj.href, e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                debug!("Failed to query VEVENTs from {}: {:?}", calendar_url, e);
                // Not an error - calendar might not have events
            }
        }

        // Fetch VTODOs (tasks)
        debug!("Querying VTODOs from: {}", calendar_url);
        match self
            .client
            .calendar_query_timerange(calendar_url, "VTODO", None, None, true)
            .await
        {
            Ok(objects) => {
                debug!("Fetched {} VTODOs from {}", objects.len(), calendar_name);
                for obj in objects {
                    if let Some(ical_data) = obj.calendar_data {
                        let etag = obj.etag.clone();
                        match ical_data.parse::<Calendar>() {
                            Ok(calendar) => {
                                for todo_comp in calendar.todos() {
                                    match parse_todo(
                                        todo_comp,
                                        calendar_name,
                                        calendar_url,
                                        etag.as_deref(),
                                    ) {
                                        Ok(todo) => todos.push(todo),
                                        Err(e) => {
                                            warn!("Failed to parse todo: {}", e);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Failed to parse iCalendar data from {}: {}", obj.href, e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                debug!("Failed to query VTODOs from {}: {:?}", calendar_url, e);
                // Not an error - calendar might not have todos
            }
        }

        Ok((events, todos))
    }
}

/// Parse an iCalendar event component into a `CalendarEvent`
fn parse_event(
    event: &Event,
    calendar_name: &str,
    calendar_url: &str,
    calendar_color: Option<&str>,
    etag: Option<&str>,
) -> Result<CalendarEvent> {
    // UID is required
    let uid = event
        .get_uid()
        .ok_or_else(|| anyhow::anyhow!("Event missing UID"))?
        .to_string();

    // Summary (title)
    let summary = event.get_summary().unwrap_or("Untitled Event").to_string();

    // Description
    let description = event.get_description().map(String::from);

    // Location
    let location = event.get_location().map(String::from);

    // Start time (required)
    let start_opt = event.get_start();
    let start = parse_datetime(start_opt.as_ref())
        .ok_or_else(|| anyhow::anyhow!("Event missing start time"))?;

    // End time (use start + 1 hour if not specified)
    let end = event.get_end().map_or_else(
        || start + chrono::Duration::hours(1),
        |end_time| {
            parse_datetime(Some(&end_time)).unwrap_or_else(|| start + chrono::Duration::hours(1))
        },
    );

    // Check if all-day event (date without time)
    let all_day = matches!(start_opt.as_ref(), Some(DatePerhapsTime::Date(_)));

    // Recurrence rule
    let rrule = event.property_value("RRULE").map(String::from);

    // Exception dates (EXDATE)
    let exdates = parse_exdates(event);

    // Status
    let status = event.get_status().map(|s| format!("{s:?}"));

    Ok(CalendarEvent {
        uid,
        summary,
        description,
        location,
        start,
        end,
        calendar_name: calendar_name.to_string(),
        calendar_url: calendar_url.to_string(),
        calendar_color: calendar_color.map(String::from),
        all_day,
        rrule,
        exdates,
        status,
        etag: etag.map(String::from),
    })
}

/// Parse an iCalendar todo component into a `Todo`
/// Parse EXDATE properties from an event
///
/// Returns a vector of `DateTime<Utc>` representing dates to exclude from recurrence
fn parse_exdates(event: &Event) -> Vec<DateTime<Utc>> {
    use icalendar::Component;

    let mut exdates = Vec::new();

    // Get all EXDATE properties (there can be multiple EXDATE lines)
    if let Some(exdate_props) = event.multi_properties().get("EXDATE") {
        for property in exdate_props {
            let value = property.value();
            // EXDATE can be a comma-separated list or a single value
            for date_str in value.split(',') {
                let trimmed = date_str.trim();

                // Try to parse as datetime with timezone info
                if let Some(dt) = parse_exdate_value(trimmed) {
                    exdates.push(dt);
                } else {
                    debug!("Failed to parse EXDATE value: {}", trimmed);
                }
            }
        }
    }

    exdates
}

/// Parse a single EXDATE value to `DateTime<Utc>`
fn parse_exdate_value(value: &str) -> Option<DateTime<Utc>> {
    // Format: YYYYMMDDTHHMMSSZ or YYYYMMDD

    // Remove any timezone prefix (e.g., "TZID=America/New_York:")
    let clean_value = value
        .find(':')
        .map_or(value, |colon_pos| &value[colon_pos + 1..]);

    // Try to parse as UTC datetime (YYYYMMDDTHHmmssZ)
    if clean_value.ends_with('Z') && clean_value.len() == 16 {
        let date_part = &clean_value[0..8];
        let time_part = &clean_value[9..15];

        if let (Ok(year), Ok(month), Ok(day), Ok(hour), Ok(min), Ok(sec)) = (
            date_part[0..4].parse::<i32>(),
            date_part[4..6].parse::<u32>(),
            date_part[6..8].parse::<u32>(),
            time_part[0..2].parse::<u32>(),
            time_part[2..4].parse::<u32>(),
            time_part[4..6].parse::<u32>(),
        ) {
            return Utc
                .with_ymd_and_hms(year, month, day, hour, min, sec)
                .single();
        }
    }

    // Try to parse as date only (YYYYMMDD) - treat as midnight UTC
    if clean_value.len() == 8
        && clean_value.chars().all(|c| c.is_ascii_digit())
        && let (Ok(year), Ok(month), Ok(day)) = (
            clean_value[0..4].parse::<i32>(),
            clean_value[4..6].parse::<u32>(),
            clean_value[6..8].parse::<u32>(),
        )
    {
        return Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).single();
    }

    // Try to parse as datetime without Z (YYYYMMDDTHHmmss)
    if clean_value.contains('T') && clean_value.len() == 15 {
        let date_part = &clean_value[0..8];
        let time_part = &clean_value[9..15];

        if let (Ok(year), Ok(month), Ok(day), Ok(hour), Ok(min), Ok(sec)) = (
            date_part[0..4].parse::<i32>(),
            date_part[4..6].parse::<u32>(),
            date_part[6..8].parse::<u32>(),
            time_part[0..2].parse::<u32>(),
            time_part[2..4].parse::<u32>(),
            time_part[4..6].parse::<u32>(),
        ) {
            return Utc
                .with_ymd_and_hms(year, month, day, hour, min, sec)
                .single();
        }
    }

    None
}

fn parse_todo(
    todo: &IcalTodo,
    calendar_name: &str,
    calendar_url: &str,
    etag: Option<&str>,
) -> Result<Todo> {
    // UID is required
    let uid = todo
        .get_uid()
        .ok_or_else(|| anyhow::anyhow!("Todo missing UID"))?
        .to_string();

    // Summary (title)
    let summary = todo.get_summary().unwrap_or("Untitled Task").to_string();

    // Description
    let description = todo.get_description().map(String::from);

    // Due date
    let due = todo.get_due().and_then(|d| parse_datetime(Some(&d)));

    // Start date
    let start = todo.get_start().and_then(|s| parse_datetime(Some(&s)));

    // Completed date
    let completed = todo.get_completed();

    // Priority (1-9, where 1 is highest)
    let priority = todo.get_priority().and_then(|p| {
        if (1..=9).contains(&p) {
            u8::try_from(p).ok()
        } else {
            None
        }
    });

    // Percent complete (0-100)
    let percent_complete = todo.get_percent_complete().and_then(|p| {
        if (0..=100).contains(&p) {
            Some(p)
        } else {
            None
        }
    });

    // Status (default to NEEDS-ACTION if not specified)
    let status = todo
        .get_status()
        .map_or_else(|| "NEEDS-ACTION".to_string(), |s| format!("{s:?}"));

    Ok(Todo {
        uid,
        summary,
        description,
        due,
        start,
        completed,
        priority,
        percent_complete,
        status,
        calendar_name: calendar_name.to_string(),
        calendar_url: calendar_url.to_string(),
        etag: etag.map(String::from),
    })
}

/// Parse a `DatePerhapsTime` into a UTC `DateTime`
///
/// All timezones are normalized to UTC for consistent storage and querying.
/// - UTC datetimes are returned as-is
/// - Floating datetimes (no timezone) are interpreted as UTC
/// - Timezone-aware datetimes are converted from their timezone to UTC
/// - Date-only values (all-day events) use midnight UTC
///
/// The consumer can convert to their preferred timezone when displaying.
fn parse_datetime(date_time: Option<&DatePerhapsTime>) -> Option<DateTime<Utc>> {
    match date_time? {
        DatePerhapsTime::DateTime(CalendarDateTime::Utc(dt)) => {
            // Already in UTC
            debug!("Parsed UTC datetime: {}", dt);
            Some(*dt)
        }
        DatePerhapsTime::DateTime(CalendarDateTime::Floating(naive)) => {
            // Floating time - treat as local timezone per iCalendar spec
            // (floating times are meant to be "in the local time of the observer")
            let dt_in_local = Local.from_local_datetime(naive).earliest()?;
            let result = dt_in_local.with_timezone(&Utc);
            debug!(
                "Parsed floating datetime (no timezone, treating as local): {} local -> {} UTC",
                naive, result
            );
            Some(result)
        }
        DatePerhapsTime::DateTime(CalendarDateTime::WithTimezone { date_time, tzid }) => {
            // Parse timezone and convert to UTC
            if let Ok(tz) = tzid.parse::<Tz>() {
                // Create a datetime in the specified timezone and convert to UTC
                let dt_in_tz = tz.from_local_datetime(date_time).earliest()?;
                let result = dt_in_tz.with_timezone(&Utc);
                debug!(
                    "Parsed datetime with timezone: {} {} -> {} UTC",
                    date_time, tzid, result
                );
                Some(result)
            } else if let Some(offset) = parse_gmt_offset(tzid) {
                // Try parsing GMT offset format (e.g., GMT-0700, GMT+0530)
                let dt_with_offset = offset.from_local_datetime(date_time).earliest()?;
                let result = dt_with_offset.with_timezone(&Utc);
                debug!(
                    "Parsed datetime with GMT offset: {} {} -> {} UTC",
                    date_time, tzid, result
                );
                Some(result)
            } else {
                // If timezone parsing fails, log a warning and treat as UTC
                warn!("Failed to parse timezone '{}', treating as UTC", tzid);
                let result = Utc.from_utc_datetime(date_time);
                debug!(
                    "Failed timezone parse: {} {} -> {} UTC (treated as UTC)",
                    date_time, tzid, result
                );
                Some(result)
            }
        }
        DatePerhapsTime::Date(d) => {
            // Date only (all-day event) - use midnight UTC
            let naive_dt = d.and_hms_opt(0, 0, 0)?;
            let result = Utc.from_utc_datetime(&naive_dt);
            debug!("Parsed date-only (all-day): {} -> {} UTC", d, result);
            Some(result)
        }
    }
}

/// Parse GMT offset format timezone (e.g., GMT-0700, GMT+0530)
/// Returns a `FixedOffset` if successful
fn parse_gmt_offset(tzid: &str) -> Option<chrono::FixedOffset> {
    // Check for GMT+/-HHMM format
    if let Some(offset_str) = tzid.strip_prefix("GMT") {
        if offset_str.is_empty() {
            // GMT with no offset is UTC (offset 0)
            return chrono::FixedOffset::east_opt(0);
        }

        // Parse sign
        let (sign, rest) = if let Some(rest) = offset_str.strip_prefix('+') {
            (1, rest)
        } else if let Some(rest) = offset_str.strip_prefix('-') {
            (-1, rest)
        } else {
            return None;
        };

        // Parse HHMM
        if rest.len() == 4 {
            let hours: i32 = rest[0..2].parse().ok()?;
            let minutes: i32 = rest[2..4].parse().ok()?;
            let total_seconds = sign * (hours * 3600 + minutes * 60);
            return chrono::FixedOffset::east_opt(total_seconds);
        }
    }

    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::{Datelike, NaiveDateTime, Timelike};

    #[test]
    fn test_parse_datetime_with_date() {
        // Create a simple date
        let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let dpt = DatePerhapsTime::Date(date);

        let result = parse_datetime(Some(&dpt));
        assert!(result.is_some());

        let dt = result.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 15);
    }

    #[test]
    fn test_parse_datetime_with_utc() {
        let naive =
            NaiveDateTime::parse_from_str("2026-01-15 14:30:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let dt = Utc.from_utc_datetime(&naive);
        let cal_dt = CalendarDateTime::Utc(dt);
        let dpt = DatePerhapsTime::DateTime(cal_dt);

        let result = parse_datetime(Some(&dpt));
        assert!(result.is_some());

        let parsed = result.unwrap();
        assert_eq!(parsed.year(), 2026);
        assert_eq!(parsed.month(), 1);
        assert_eq!(parsed.day(), 15);
        assert_eq!(parsed.hour(), 14);
        assert_eq!(parsed.minute(), 30);
    }

    #[test]
    fn test_parse_datetime_with_floating() {
        let naive =
            NaiveDateTime::parse_from_str("2026-01-15 10:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let cal_dt = CalendarDateTime::Floating(naive);
        let dpt = DatePerhapsTime::DateTime(cal_dt);

        let result = parse_datetime(Some(&dpt));
        assert!(result.is_some());

        let parsed = result.unwrap();
        // Floating times are now treated as local time and converted to UTC
        // So we can't assert a specific hour (depends on system timezone)
        // But we can verify the date and that it was converted
        assert_eq!(parsed.year(), 2026);
        assert_eq!(parsed.month(), 1);
        // Day might shift depending on timezone, but should be 14, 15, or 16
        assert!(parsed.day() >= 14 && parsed.day() <= 16);
        assert_eq!(parsed.minute(), 0);
    }

    #[test]
    fn test_parse_datetime_none() {
        let result = parse_datetime(None);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_event_basic() {
        use icalendar::Event;

        let event = Event::new()
            .uid("event-123")
            .summary("Test Event")
            .description("This is a test")
            .location("Test Location")
            .starts(Utc.with_ymd_and_hms(2026, 3, 15, 10, 0, 0).unwrap())
            .ends(Utc.with_ymd_and_hms(2026, 3, 15, 11, 0, 0).unwrap())
            .done();

        let result = parse_event(&event, "Test Calendar", "/calendar/test", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.uid, "event-123");
        assert_eq!(parsed.summary, "Test Event");
        assert_eq!(parsed.description, Some("This is a test".to_string()));
        assert_eq!(parsed.location, Some("Test Location".to_string()));
        assert_eq!(parsed.calendar_name, "Test Calendar");
        assert_eq!(parsed.calendar_url, "/calendar/test");
        assert!(!parsed.all_day);
        assert_eq!(parsed.etag, None);
    }

    #[test]
    fn test_parse_event_with_etag() {
        use icalendar::Event;

        let event = Event::new()
            .uid("event-456")
            .summary("Event with ETag")
            .starts(Utc.with_ymd_and_hms(2026, 4, 1, 9, 0, 0).unwrap())
            .ends(Utc.with_ymd_and_hms(2026, 4, 1, 10, 0, 0).unwrap())
            .done();

        let result = parse_event(&event, "Calendar", "/cal", None, Some("etag-123"));
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.etag, Some("etag-123".to_string()));
    }

    #[test]
    fn test_parse_event_all_day() {
        use icalendar::Event;

        let date = chrono::NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
        let event = Event::new()
            .uid("all-day-1")
            .summary("All Day Event")
            .all_day(date)
            .done();

        let result = parse_event(&event, "Calendar", "/cal", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.summary, "All Day Event");
        assert!(parsed.all_day);
    }

    #[test]
    fn test_parse_event_minimal() {
        use icalendar::Event;

        let event = Event::new()
            .uid("minimal-1")
            .starts(Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap())
            .done();

        let result = parse_event(&event, "Cal", "/c", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.summary, "Untitled Event");
        assert_eq!(parsed.description, None);
        assert_eq!(parsed.location, None);
    }

    #[test]
    fn test_parse_todo_basic() {
        use icalendar::Todo as IcalTodo;

        let todo = IcalTodo::new()
            .uid("todo-123")
            .summary("Test Task")
            .description("Do this thing")
            .due(Utc.with_ymd_and_hms(2026, 3, 20, 17, 0, 0).unwrap())
            .priority(5)
            .percent_complete(50)
            .done();

        let result = parse_todo(&todo, "Tasks", "/tasks", None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.uid, "todo-123");
        assert_eq!(parsed.summary, "Test Task");
        assert_eq!(parsed.description, Some("Do this thing".to_string()));
        assert!(parsed.due.is_some());
        assert_eq!(parsed.priority, Some(5));
        assert_eq!(parsed.percent_complete, Some(50));
        assert_eq!(parsed.calendar_name, "Tasks");
        assert_eq!(parsed.calendar_url, "/tasks");
    }

    #[test]
    fn test_parse_todo_minimal() {
        use icalendar::Todo as IcalTodo;

        let todo = IcalTodo::new().uid("todo-min").done();

        let result = parse_todo(&todo, "Tasks", "/t", None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.summary, "Untitled Task");
        assert_eq!(parsed.description, None);
        assert_eq!(parsed.due, None);
        assert_eq!(parsed.start, None);
        assert_eq!(parsed.completed, None);
        assert_eq!(parsed.priority, None);
        assert_eq!(parsed.percent_complete, None);
        assert_eq!(parsed.status, "NEEDS-ACTION");
    }

    #[test]
    fn test_parse_todo_completed() {
        use icalendar::Todo as IcalTodo;

        let completed_time = Utc.with_ymd_and_hms(2026, 3, 1, 15, 30, 0).unwrap();
        let todo = IcalTodo::new()
            .uid("todo-done")
            .summary("Completed Task")
            .completed(completed_time)
            .percent_complete(100)
            .done();

        let result = parse_todo(&todo, "Tasks", "/tasks", Some("etag-456"));
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.summary, "Completed Task");
        assert_eq!(parsed.completed, Some(completed_time));
        assert_eq!(parsed.percent_complete, Some(100));
        assert_eq!(parsed.etag, Some("etag-456".to_string()));
    }

    #[test]
    fn test_parse_gmt_offset() {
        // Test GMT-0700 (Mountain Time)
        let offset = parse_gmt_offset("GMT-0700");
        assert!(offset.is_some());
        assert_eq!(offset.unwrap().local_minus_utc(), -7 * 3600);

        // Test GMT+0530 (India)
        let offset = parse_gmt_offset("GMT+0530");
        assert!(offset.is_some());
        assert_eq!(offset.unwrap().local_minus_utc(), 5 * 3600 + 30 * 60);

        // Test GMT (UTC)
        let offset = parse_gmt_offset("GMT");
        assert!(offset.is_some());
        assert_eq!(offset.unwrap().local_minus_utc(), 0);

        // Test invalid format
        let offset = parse_gmt_offset("America/Denver");
        assert!(offset.is_none());

        let offset = parse_gmt_offset("GMT-07");
        assert!(offset.is_none());
    }

    #[test]
    fn test_parse_todo_priority_bounds() {
        use icalendar::Todo as IcalTodo;

        // Valid priority (1-9)
        let todo = IcalTodo::new().uid("p1").priority(1).done();
        let result = parse_todo(&todo, "T", "/t", None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().priority, Some(1));

        // Priority 9 (edge case)
        let todo = IcalTodo::new().uid("p9").priority(9).done();
        let result = parse_todo(&todo, "T", "/t", None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().priority, Some(9));
    }

    #[test]
    fn test_parse_todo_percent_complete_bounds() {
        use icalendar::Todo as IcalTodo;

        // 0% complete
        let todo = IcalTodo::new().uid("pc0").percent_complete(0).done();
        let result = parse_todo(&todo, "T", "/t", None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().percent_complete, Some(0));

        // 100% complete
        let todo = IcalTodo::new().uid("pc100").percent_complete(100).done();
        let result = parse_todo(&todo, "T", "/t", None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().percent_complete, Some(100));

        // 50% complete
        let todo = IcalTodo::new().uid("pc50").percent_complete(50).done();
        let result = parse_todo(&todo, "T", "/t", None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().percent_complete, Some(50));
    }

    #[test]
    fn test_parse_datetime_with_timezone() {
        let naive =
            NaiveDateTime::parse_from_str("2026-07-04 18:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let cal_dt = CalendarDateTime::WithTimezone {
            date_time: naive,
            tzid: "America/New_York".to_string(),
        };
        let dpt = DatePerhapsTime::DateTime(cal_dt);

        let result = parse_datetime(Some(&dpt));
        assert!(result.is_some());

        let parsed = result.unwrap();
        // 18:00 in America/New_York (EDT in July) is 22:00 UTC
        assert_eq!(parsed.year(), 2026);
        assert_eq!(parsed.month(), 7);
        assert_eq!(parsed.day(), 4);
        assert_eq!(parsed.hour(), 22);
        assert_eq!(parsed.minute(), 0);
    }

    #[test]
    fn test_parse_datetime_with_invalid_timezone() {
        let naive =
            NaiveDateTime::parse_from_str("2026-07-04 18:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let cal_dt = CalendarDateTime::WithTimezone {
            date_time: naive,
            tzid: "Invalid/Timezone".to_string(),
        };
        let dpt = DatePerhapsTime::DateTime(cal_dt);

        let result = parse_datetime(Some(&dpt));
        assert!(result.is_some());

        // Should fall back to treating as UTC
        let parsed = result.unwrap();
        assert_eq!(parsed.hour(), 18);
    }

    #[test]
    fn test_parse_datetime_with_gmt_offset_timezone() {
        let naive =
            NaiveDateTime::parse_from_str("2026-07-04 18:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let cal_dt = CalendarDateTime::WithTimezone {
            date_time: naive,
            tzid: "GMT-0700".to_string(),
        };
        let dpt = DatePerhapsTime::DateTime(cal_dt);

        let result = parse_datetime(Some(&dpt));
        assert!(result.is_some());

        let parsed = result.unwrap();
        // 18:00 GMT-0700 is 01:00 UTC next day
        assert_eq!(parsed.year(), 2026);
        assert_eq!(parsed.month(), 7);
        assert_eq!(parsed.day(), 5);
        assert_eq!(parsed.hour(), 1);
    }

    #[test]
    fn test_parse_event_with_calendar_color() {
        use icalendar::Event;

        let event = Event::new()
            .uid("event-color")
            .summary("Colored Event")
            .starts(Utc.with_ymd_and_hms(2026, 3, 15, 10, 0, 0).unwrap())
            .ends(Utc.with_ymd_and_hms(2026, 3, 15, 11, 0, 0).unwrap())
            .done();

        let result = parse_event(&event, "Calendar", "/cal", Some("#FF5733"), None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.calendar_color, Some("#FF5733".to_string()));
    }

    #[test]
    fn test_parse_event_with_rrule() {
        use icalendar::Event;

        let event = Event::new()
            .uid("recurring-event")
            .summary("Weekly Meeting")
            .starts(Utc.with_ymd_and_hms(2026, 3, 15, 10, 0, 0).unwrap())
            .ends(Utc.with_ymd_and_hms(2026, 3, 15, 11, 0, 0).unwrap())
            .add_property("RRULE", "FREQ=WEEKLY;BYDAY=MO")
            .done();

        let result = parse_event(&event, "Calendar", "/cal", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.rrule, Some("FREQ=WEEKLY;BYDAY=MO".to_string()));
    }

    #[test]
    fn test_parse_event_with_status() {
        use icalendar::Event;

        let event = Event::new()
            .uid("confirmed-event")
            .summary("Confirmed Meeting")
            .starts(Utc.with_ymd_and_hms(2026, 3, 15, 10, 0, 0).unwrap())
            .ends(Utc.with_ymd_and_hms(2026, 3, 15, 11, 0, 0).unwrap())
            .status(icalendar::EventStatus::Confirmed)
            .done();

        let result = parse_event(&event, "Calendar", "/cal", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.status, Some("Confirmed".to_string()));
    }

    #[test]
    fn test_parse_event_without_end_time() {
        use icalendar::Event;

        let event = Event::new()
            .uid("no-end-event")
            .summary("Event without end")
            .starts(Utc.with_ymd_and_hms(2026, 3, 15, 10, 0, 0).unwrap())
            .done();

        let result = parse_event(&event, "Calendar", "/cal", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        // Should default to 1 hour after start
        assert_eq!(
            parsed.start,
            Utc.with_ymd_and_hms(2026, 3, 15, 10, 0, 0).unwrap()
        );
        assert_eq!(
            parsed.end,
            Utc.with_ymd_and_hms(2026, 3, 15, 11, 0, 0).unwrap()
        );
    }

    #[test]
    fn test_parse_todo_with_status_completed() {
        use icalendar::Todo as IcalTodo;

        let todo = IcalTodo::new()
            .uid("completed-todo")
            .summary("Done Task")
            .status(icalendar::TodoStatus::Completed)
            .done();

        let result = parse_todo(&todo, "Tasks", "/tasks", None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.status, "Completed");
    }

    #[test]
    fn test_parse_todo_with_status_in_process() {
        use icalendar::Todo as IcalTodo;

        let todo = IcalTodo::new()
            .uid("in-progress-todo")
            .summary("Working Task")
            .status(icalendar::TodoStatus::InProcess)
            .done();

        let result = parse_todo(&todo, "Tasks", "/tasks", None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.status, "InProcess");
    }

    #[test]
    fn test_parse_todo_with_start_date() {
        use icalendar::Todo as IcalTodo;

        let start_time = Utc.with_ymd_and_hms(2026, 3, 1, 9, 0, 0).unwrap();
        let todo = IcalTodo::new()
            .uid("scheduled-todo")
            .summary("Scheduled Task")
            .starts(start_time)
            .done();

        let result = parse_todo(&todo, "Tasks", "/tasks", None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.start, Some(start_time));
    }

    #[test]
    fn test_parse_gmt_offset_edge_cases() {
        // Test GMT with positive offset at boundary
        let offset = parse_gmt_offset("GMT+1400");
        assert!(offset.is_some());
        assert_eq!(offset.unwrap().local_minus_utc(), 14 * 3600);

        // Test GMT with negative offset at boundary
        let offset = parse_gmt_offset("GMT-1200");
        assert!(offset.is_some());
        assert_eq!(offset.unwrap().local_minus_utc(), -12 * 3600);

        // Test invalid - not enough digits
        let offset = parse_gmt_offset("GMT+05");
        assert!(offset.is_none());

        // Test invalid - too many digits
        let offset = parse_gmt_offset("GMT+05300");
        assert!(offset.is_none());

        // Test invalid - no GMT prefix
        let offset = parse_gmt_offset("+0530");
        assert!(offset.is_none());
    }

    #[test]
    fn test_parse_event_with_multiple_properties() {
        use icalendar::Event;

        let event = Event::new()
            .uid("complex-event")
            .summary("Complex Event")
            .description("Detailed description")
            .location("Conference Room")
            .starts(Utc.with_ymd_and_hms(2026, 5, 1, 14, 0, 0).unwrap())
            .ends(Utc.with_ymd_and_hms(2026, 5, 1, 16, 0, 0).unwrap())
            .status(icalendar::EventStatus::Confirmed)
            .add_property("RRULE", "FREQ=MONTHLY")
            .done();

        let result = parse_event(
            &event,
            "Work Calendar",
            "/calendars/work/",
            Some("#0000FF"),
            Some("etag-xyz"),
        );
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.uid, "complex-event");
        assert_eq!(parsed.summary, "Complex Event");
        assert_eq!(parsed.description, Some("Detailed description".to_string()));
        assert_eq!(parsed.location, Some("Conference Room".to_string()));
        assert_eq!(parsed.calendar_name, "Work Calendar");
        assert_eq!(parsed.calendar_url, "/calendars/work/");
        assert_eq!(parsed.calendar_color, Some("#0000FF".to_string()));
        assert_eq!(parsed.rrule, Some("FREQ=MONTHLY".to_string()));
        assert_eq!(parsed.status, Some("Confirmed".to_string()));
        assert_eq!(parsed.etag, Some("etag-xyz".to_string()));
        assert!(!parsed.all_day);
    }

    #[test]
    fn test_parse_todo_with_all_fields() {
        use icalendar::Todo as IcalTodo;

        let start_time = Utc.with_ymd_and_hms(2026, 4, 1, 9, 0, 0).unwrap();
        let due_time = Utc.with_ymd_and_hms(2026, 4, 5, 17, 0, 0).unwrap();
        let completed_time = Utc.with_ymd_and_hms(2026, 4, 4, 15, 30, 0).unwrap();

        let todo = IcalTodo::new()
            .uid("full-todo")
            .summary("Complete Task")
            .description("Task with all fields")
            .starts(start_time)
            .due(due_time)
            .completed(completed_time)
            .priority(3)
            .percent_complete(100)
            .status(icalendar::TodoStatus::Completed)
            .done();

        let result = parse_todo(&todo, "My Tasks", "/calendars/tasks/", Some("etag-abc"));
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.uid, "full-todo");
        assert_eq!(parsed.summary, "Complete Task");
        assert_eq!(parsed.description, Some("Task with all fields".to_string()));
        assert_eq!(parsed.start, Some(start_time));
        assert_eq!(parsed.due, Some(due_time));
        assert_eq!(parsed.completed, Some(completed_time));
        assert_eq!(parsed.priority, Some(3));
        assert_eq!(parsed.percent_complete, Some(100));
        assert_eq!(parsed.status, "Completed");
        assert_eq!(parsed.calendar_name, "My Tasks");
        assert_eq!(parsed.calendar_url, "/calendars/tasks/");
        assert_eq!(parsed.etag, Some("etag-abc".to_string()));
    }

    #[test]
    fn test_parse_datetime_with_timezone_tz() {
        let naive =
            NaiveDateTime::parse_from_str("2026-01-15 14:30:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let tz: Tz = "America/New_York".parse().unwrap();
        let _dt = tz.from_local_datetime(&naive).unwrap();
        let cal_dt = CalendarDateTime::WithTimezone {
            date_time: naive,
            tzid: "America/New_York".to_string(),
        };
        let dpt = DatePerhapsTime::DateTime(cal_dt);

        let result = parse_datetime(Some(&dpt));
        assert!(result.is_some());

        let parsed = result.unwrap();
        assert_eq!(parsed.year(), 2026);
        assert_eq!(parsed.month(), 1);
        assert_eq!(parsed.day(), 15);
    }

    #[test]
    fn test_parse_gmt_offset_positive() {
        use chrono::FixedOffset;
        assert_eq!(
            parse_gmt_offset("GMT+0500"),
            FixedOffset::east_opt(5 * 3600)
        );
        assert_eq!(
            parse_gmt_offset("GMT+0530"),
            FixedOffset::east_opt(5 * 3600 + 30 * 60)
        );
        assert_eq!(parse_gmt_offset("GMT+0000"), FixedOffset::east_opt(0));
        assert_eq!(parse_gmt_offset("GMT"), FixedOffset::east_opt(0));
    }

    #[test]
    fn test_parse_gmt_offset_negative() {
        use chrono::FixedOffset;
        assert_eq!(
            parse_gmt_offset("GMT-0500"),
            FixedOffset::east_opt(-5 * 3600)
        );
        assert_eq!(
            parse_gmt_offset("GMT-0800"),
            FixedOffset::east_opt(-8 * 3600)
        );
    }

    #[test]
    fn test_parse_gmt_offset_invalid() {
        assert_eq!(parse_gmt_offset("invalid"), None);
        assert_eq!(parse_gmt_offset("GMT+99"), None);
        assert_eq!(parse_gmt_offset("+0500"), None); // Missing GMT prefix
    }

    #[test]
    fn test_parse_event_no_summary() {
        let start_time = Utc.with_ymd_and_hms(2026, 1, 10, 10, 0, 0).unwrap();
        let end_time = Utc.with_ymd_and_hms(2026, 1, 10, 11, 0, 0).unwrap();

        let event = Event::new()
            .uid("no-summary")
            .starts(start_time)
            .ends(end_time)
            .done();

        let result = parse_event(&event, "Test", "/test", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.summary, "Untitled Event");
    }

    #[test]
    fn test_parse_todo_no_summary() {
        use icalendar::Todo as IcalTodo;

        let todo = IcalTodo::new().uid("no-summary").done();

        let result = parse_todo(&todo, "Tasks", "/tasks", None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.summary, "Untitled Task");
    }

    #[test]
    fn test_parse_event_with_description() {
        let start_time = Utc.with_ymd_and_hms(2026, 2, 1, 9, 0, 0).unwrap();
        let end_time = Utc.with_ymd_and_hms(2026, 2, 1, 10, 0, 0).unwrap();

        let event = Event::new()
            .uid("with-desc")
            .summary("Test Event")
            .description("This is a test description")
            .starts(start_time)
            .ends(end_time)
            .done();

        let result = parse_event(&event, "Calendar", "/cal", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(
            parsed.description,
            Some("This is a test description".to_string())
        );
    }

    #[test]
    fn test_parse_event_with_location() {
        let start_time = Utc.with_ymd_and_hms(2026, 3, 1, 14, 0, 0).unwrap();
        let end_time = Utc.with_ymd_and_hms(2026, 3, 1, 15, 0, 0).unwrap();

        let event = Event::new()
            .uid("with-loc")
            .summary("Meeting")
            .location("Conference Room A")
            .starts(start_time)
            .ends(end_time)
            .done();

        let result = parse_event(&event, "Work", "/work", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.location, Some("Conference Room A".to_string()));
    }

    #[test]
    fn test_parse_todo_with_description() {
        use icalendar::Todo as IcalTodo;

        let todo = IcalTodo::new()
            .uid("todo-desc")
            .summary("Task")
            .description("Task description")
            .done();

        let result = parse_todo(&todo, "Tasks", "/tasks", None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.description, Some("Task description".to_string()));
    }

    #[test]
    fn test_parse_todo_cancelled_status() {
        use icalendar::Todo as IcalTodo;

        let todo = IcalTodo::new()
            .uid("cancelled")
            .summary("Cancelled Task")
            .status(icalendar::TodoStatus::Cancelled)
            .done();

        let result = parse_todo(&todo, "Tasks", "/tasks", None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.status, "Cancelled");
    }

    #[test]
    fn test_parse_todo_in_process_status() {
        use icalendar::Todo as IcalTodo;

        let todo = IcalTodo::new()
            .uid("in-progress")
            .summary("Active Task")
            .status(icalendar::TodoStatus::InProcess)
            .done();

        let result = parse_todo(&todo, "Tasks", "/tasks", None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.status, "InProcess");
    }

    #[test]
    fn test_parse_event_tentative_status() {
        let start_time = Utc.with_ymd_and_hms(2026, 4, 1, 10, 0, 0).unwrap();
        let end_time = Utc.with_ymd_and_hms(2026, 4, 1, 11, 0, 0).unwrap();

        let event = Event::new()
            .uid("tentative")
            .summary("Maybe Event")
            .starts(start_time)
            .ends(end_time)
            .status(icalendar::EventStatus::Tentative)
            .done();

        let result = parse_event(&event, "Cal", "/cal", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.status, Some("Tentative".to_string()));
    }

    #[test]
    fn test_parse_event_with_exdate() {
        let ical_str = r"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:test-exdate
DTSTART:20260101T100000Z
DTEND:20260101T110000Z
SUMMARY:Event with EXDATE
RRULE:FREQ=WEEKLY;COUNT=4
EXDATE:20260108T100000Z
END:VEVENT
END:VCALENDAR";

        let calendar = ical_str.parse::<Calendar>().unwrap();
        let events: Vec<_> = calendar.events().collect();
        assert_eq!(events.len(), 1);

        let result = parse_event(events[0], "Test", "/test", None, None);
        assert!(result.is_ok());

        let event = result.unwrap();
        assert_eq!(event.uid, "test-exdate");
        assert_eq!(event.rrule, Some("FREQ=WEEKLY;COUNT=4".to_string()));
        assert_eq!(event.exdates.len(), 1);

        // Verify the EXDATE was parsed correctly
        let expected_exdate = Utc.with_ymd_and_hms(2026, 1, 8, 10, 0, 0).unwrap();
        assert_eq!(event.exdates[0], expected_exdate);
    }

    #[test]
    fn test_parse_event_cancelled_status() {
        let start_time = Utc.with_ymd_and_hms(2026, 5, 1, 10, 0, 0).unwrap();
        let end_time = Utc.with_ymd_and_hms(2026, 5, 1, 11, 0, 0).unwrap();

        let event = Event::new()
            .uid("cancelled")
            .summary("Cancelled Event")
            .starts(start_time)
            .ends(end_time)
            .status(icalendar::EventStatus::Cancelled)
            .done();

        let result = parse_event(&event, "Cal", "/cal", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.status, Some("Cancelled".to_string()));
    }

    #[test]
    fn test_parse_todo_needs_action_status() {
        use icalendar::Todo as IcalTodo;

        let todo = IcalTodo::new()
            .uid("needs-action")
            .summary("New Task")
            .status(icalendar::TodoStatus::NeedsAction)
            .done();

        let result = parse_todo(&todo, "Tasks", "/tasks", None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.status, "NeedsAction");
    }

    #[test]
    fn test_batch_size_constant() {
        assert_eq!(BATCH_SIZE, 500);
    }

    #[test]
    fn test_parse_event_all_day_with_date_only() {
        // All-day events typically use Date type
        let date = chrono::NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
        let dpt_start = DatePerhapsTime::Date(date);
        let dpt_end = DatePerhapsTime::Date(date + chrono::Days::new(1));

        let event = Event::new()
            .uid("all-day-date")
            .summary("All Day Event")
            .starts(dpt_start)
            .ends(dpt_end)
            .done();

        let result = parse_event(&event, "Cal", "/cal", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert!(parsed.all_day);
        assert_eq!(parsed.start.day(), 15);
    }

    #[test]
    fn test_parse_todo_no_dates() {
        use icalendar::Todo as IcalTodo;

        let todo = IcalTodo::new()
            .uid("no-dates")
            .summary("Task without dates")
            .done();

        let result = parse_todo(&todo, "Tasks", "/tasks", Some("etag-xyz"));
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert!(parsed.due.is_none());
        assert!(parsed.start.is_none());
        assert!(parsed.completed.is_none());
        assert_eq!(parsed.etag, Some("etag-xyz".to_string()));
    }

    #[test]
    fn test_parse_event_no_etag() {
        let start_time = Utc.with_ymd_and_hms(2026, 7, 1, 10, 0, 0).unwrap();
        let end_time = Utc.with_ymd_and_hms(2026, 7, 1, 11, 0, 0).unwrap();

        let event = Event::new()
            .uid("no-etag")
            .summary("Event")
            .starts(start_time)
            .ends(end_time)
            .done();

        let result = parse_event(&event, "Cal", "/cal", None, None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert!(parsed.etag.is_none());
    }

    #[test]
    fn test_parse_todo_priority_zero() {
        use icalendar::Todo as IcalTodo;

        let todo = IcalTodo::new()
            .uid("priority-zero")
            .summary("Undefined Priority")
            .priority(0)
            .done();

        let result = parse_todo(&todo, "Tasks", "/tasks", None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        // Priority 0 means undefined, so it should be None
        assert_eq!(parsed.priority, None);
    }

    #[test]
    fn test_parse_todo_percent_zero() {
        use icalendar::Todo as IcalTodo;

        let todo = IcalTodo::new()
            .uid("percent-zero")
            .summary("Not Started")
            .percent_complete(0)
            .done();

        let result = parse_todo(&todo, "Tasks", "/tasks", None);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        assert_eq!(parsed.percent_complete, Some(0));
    }

    #[test]
    fn test_parse_datetime_date_becomes_midnight_utc() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 8, 20).unwrap();
        let dpt = DatePerhapsTime::Date(date);

        let result = parse_datetime(Some(&dpt));
        assert!(result.is_some());

        let parsed = result.unwrap();
        assert_eq!(parsed.hour(), 0);
        assert_eq!(parsed.minute(), 0);
        assert_eq!(parsed.second(), 0);
    }

    // Full integration tests for sync manager are in the integration test suite
}
