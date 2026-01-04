// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::cache::CacheManager;
use crate::models::{CalendarData, CalendarEvent, Todo};
use anyhow::Result;
use chrono::{DateTime, Local, TimeZone, Utc};
use chrono_tz::Tz;
use fast_dav_rs::CalDavClient;
use icalendar::{
    Calendar, CalendarDateTime, Component, DatePerhapsTime, Event, EventLike, Todo as IcalTodo,
};
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

        // Check if server supports WebDAV sync
        let supports_sync = self.client.supports_webdav_sync().await.unwrap_or(false);
        if supports_sync {
            info!("Server supports WebDAV sync - using incremental updates");
        } else {
            info!("Server does not support WebDAV sync - using full sync");
        }

        // Process each calendar
        for calendar in &calendars {
            let calendar_name = calendar
                .displayname
                .clone()
                .unwrap_or_else(|| "Unnamed".to_string());
            let calendar_url = calendar.href.clone();

            debug!("Syncing calendar: {}", calendar_name);

            // Check if this calendar has been synced before
            let has_been_synced = {
                let data = self.data.read().await;
                // Calendar has been synced if we have any events/todos from it
                data.events.iter().any(|e| e.calendar_url == calendar_url)
                    || data.todos.iter().any(|t| t.calendar_url == calendar_url)
                    || data.sync_tokens.contains_key(&calendar_url)
            };

            // Strategy:
            // 1. First sync ever: Use full query (most reliable for initial data fetch)
            // 2. All subsequent syncs: Use sync_collection if server supports it
            let sync_result = if !has_been_synced {
                debug!("First sync for {} - using full query", calendar_name);
                self.sync_calendar_full(&calendar_url, &calendar_name).await
            } else if supports_sync {
                debug!(
                    "Using sync_collection for {} (subsequent sync)",
                    calendar_name
                );
                self.sync_calendar_incremental(&calendar_url, &calendar_name)
                    .await
            } else {
                debug!(
                    "Using full sync for {} (no WebDAV sync support)",
                    calendar_name
                );
                self.sync_calendar_full(&calendar_url, &calendar_name).await
            };

            // Log any errors but continue with other calendars
            if let Err(e) = sync_result {
                error!(
                    "Failed to sync calendar {} at {}: {:?}",
                    calendar_name, calendar_url, e
                );
            }
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

        debug!(
            "Incremental sync for {} with token: {:?}",
            calendar_name,
            sync_token.as_ref().map_or("none", |_| "present")
        );

        // Perform sync-collection query
        let sync_response = self
            .client
            .sync_collection(calendar_url, sync_token.as_deref(), None, true)
            .await?;

        debug!(
            "sync_collection returned {} items for {}",
            sync_response.items.len(),
            calendar_name
        );

        let mut added_events = 0;
        let mut added_todos = 0;
        let mut deleted_count = 0;

        // Process each changed item
        for item in &sync_response.items {
            debug!(
                "Processing item: href={}, is_deleted={}, has_data={}",
                item.href,
                item.is_deleted,
                item.calendar_data.is_some()
            );

            if item.is_deleted {
                deleted_count += self.process_deleted_item(&item.href).await;
            } else if let Some(ical_data) = &item.calendar_data {
                let (events, todos) = self
                    .process_calendar_item(
                        ical_data,
                        &item.href,
                        item.etag.as_ref(),
                        calendar_name,
                        calendar_url,
                    )
                    .await;
                added_events += events;
                added_todos += todos;
            } else {
                debug!("Item {} has no calendar data", item.href);
            }
        }

        // Store new sync token
        if let Some(new_token) = sync_response.sync_token {
            let mut data = self.data.write().await;
            data.sync_tokens.insert(calendar_url.to_string(), new_token);
            drop(data);
        }

        info!(
            "Incremental sync for {}: +{} events, +{} todos, -{} deleted",
            calendar_name, added_events, added_todos, deleted_count
        );

        Ok(())
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

        // Mark this calendar as synced by adding an empty token
        // This signals that next sync should try sync_collection
        if self.client.supports_webdav_sync().await.unwrap_or(false) {
            data.sync_tokens
                .entry(calendar_url.to_string())
                .or_insert_with(String::new);
        }

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
                    match parse_event(
                        event_comp,
                        calendar_name,
                        calendar_url,
                        etag.map(String::as_str),
                    ) {
                        Ok(event) => {
                            data.events.retain(|e| e.uid != event.uid);
                            data.events.push(event);
                            events_added += 1;
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
                                    match parse_event(
                                        event_comp,
                                        calendar_name,
                                        calendar_url,
                                        etag.as_deref(),
                                    ) {
                                        Ok(event) => events.push(event),
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
    debug!("Raw event start time for '{}': {:?}", summary, start_opt);
    let start = parse_datetime(start_opt.as_ref())
        .ok_or_else(|| anyhow::anyhow!("Event missing start time"))?;

    // End time (use start + 1 hour if not specified)
    let end_opt = event.get_end();
    debug!("Raw event end time for '{}': {:?}", summary, end_opt);
    let end = end_opt.map_or_else(
        || start + chrono::Duration::hours(1),
        |end_time| {
            parse_datetime(Some(&end_time)).unwrap_or_else(|| start + chrono::Duration::hours(1))
        },
    );

    // Check if all-day event (date without time)
    let all_day = matches!(start_opt.as_ref(), Some(DatePerhapsTime::Date(_)));

    // Recurrence rule
    let rrule = event.property_value("RRULE").map(String::from);

    // Status
    let status = event.get_status().map(|s| format!("{s:?}"));

    info!(
        "Parsed event: '{}' | Start: {} UTC | End: {} UTC | All-day: {} | Calendar: {}",
        summary, start, end, all_day, calendar_name
    );

    Ok(CalendarEvent {
        uid,
        summary,
        description,
        location,
        start,
        end,
        calendar_name: calendar_name.to_string(),
        calendar_url: calendar_url.to_string(),
        all_day,
        rrule,
        status,
        etag: etag.map(String::from),
    })
}

/// Parse an iCalendar todo component into a `Todo`
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
    let due_opt = todo.get_due();
    debug!("Raw todo due time for '{}': {:?}", summary, due_opt);
    let due = due_opt.and_then(|d| parse_datetime(Some(&d)));

    // Start date
    let start_opt = todo.get_start();
    debug!("Raw todo start time for '{}': {:?}", summary, start_opt);
    let start = start_opt.and_then(|s| parse_datetime(Some(&s)));

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

    info!(
        "Parsed todo: '{}' | Due: {:?} UTC | Start: {:?} UTC | Status: {} | Calendar: {}",
        summary, due, start, status, calendar_name
    );

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

        let result = parse_event(&event, "Test Calendar", "/calendar/test", None);
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

        let result = parse_event(&event, "Calendar", "/cal", Some("etag-123"));
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

        let result = parse_event(&event, "Calendar", "/cal", None);
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

        let result = parse_event(&event, "Cal", "/c", None);
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

    // Full integration tests for sync manager are in the integration test suite
}
