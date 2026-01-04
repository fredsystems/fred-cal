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
use icalendar::{
    Calendar, CalendarDateTime, Component, DatePerhapsTime, Event, EventLike, Todo as IcalTodo,
};
use quick_xml::Reader;
use quick_xml::events::Event as XmlEvent;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, interval};

/// Manages synchronization with `CalDAV` server
pub struct SyncManager {
    client: Arc<CalDavClient>,
    cache: Arc<CacheManager>,
    data: Arc<RwLock<CalendarData>>,
    calendar_colors: Arc<RwLock<std::collections::HashMap<String, String>>>,
    server_url: String,
    username: String,
    password: String,
}

impl SyncManager {
    /// Create a new sync manager
    ///
    /// # Errors
    ///
    /// Returns an error if the cache cannot be loaded from disk.
    pub fn new(
        client: CalDavClient,
        cache: CacheManager,
        server_url: String,
        username: String,
        password: String,
    ) -> Result<Self> {
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
            calendar_colors: Arc::new(RwLock::new(std::collections::HashMap::new())),
            server_url,
            username,
            password,
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

        // Track calendar URLs we see during this sync
        let mut active_calendar_urls = std::collections::HashSet::new();

        // Process each calendar
        for calendar in &calendars {
            let calendar_name = calendar
                .displayname
                .clone()
                .unwrap_or_else(|| "Unnamed".to_string());
            let calendar_url = calendar.href.clone();

            // Track this calendar as active
            active_calendar_urls.insert(calendar_url.clone());

            // Store calendar color if available
            if let Some(color) = &calendar.color {
                self.calendar_colors
                    .write()
                    .await
                    .insert(calendar_url.clone(), color.clone());
                debug!("Calendar '{}' has color: {}", calendar_name, color);
            } else {
                // Try fetching color from Apple namespace if standard namespace didn't provide it
                if let Ok(Some(apple_color)) = self.fetch_apple_calendar_color(&calendar_url).await
                {
                    self.calendar_colors
                        .write()
                        .await
                        .insert(calendar_url.clone(), apple_color.clone());
                    debug!(
                        "Calendar '{}' has Apple calendar-color: {}",
                        calendar_name, apple_color
                    );
                }
            }

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

    /// Fetch calendar color from Apple namespace (<http://apple.com/ns/ical/>)
    ///
    /// This is a fallback for servers (like iCloud) that provide calendar-color
    /// in Apple's proprietary namespace instead of the standard `CalDAV` namespace.
    ///
    /// # Errors
    ///
    /// Returns an error if the PROPFIND request fails
    async fn fetch_apple_calendar_color(&self, calendar_url: &str) -> Result<Option<String>> {
        // Construct absolute URL from the relative calendar path
        let absolute_url =
            if calendar_url.starts_with("http://") || calendar_url.starts_with("https://") {
                calendar_url.to_string()
            } else {
                let base = self.server_url.trim_end_matches('/');
                let path = calendar_url.trim_start_matches('/');
                format!("{base}/{path}")
            };

        let propfind_body = r#"<?xml version="1.0" encoding="UTF-8"?>
<d:propfind xmlns:d="DAV:" xmlns:apple="http://apple.com/ns/ical/">
  <d:prop>
    <apple:calendar-color/>
  </d:prop>
</d:propfind>"#;

        let client = reqwest::Client::new();
        let response = client
            .request(reqwest::Method::from_bytes(b"PROPFIND")?, &absolute_url)
            .header("Depth", "0")
            .header("Content-Type", "application/xml; charset=utf-8")
            .basic_auth(&self.username, Some(&self.password))
            .body(propfind_body)
            .send()
            .await?;

        if !response.status().is_success() {
            debug!(
                "Apple color PROPFIND returned non-success status: {}",
                response.status()
            );
            return Ok(None);
        }

        let body = response.text().await?;

        // Parse the XML to extract Apple calendar-color
        let mut reader = Reader::from_str(&body);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut in_apple_color = false;
        let mut color_value = None;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(XmlEvent::Start(ref e)) => {
                    let name = e.name();
                    let local_name_bytes = name.local_name();
                    let local_name = String::from_utf8_lossy(local_name_bytes.as_ref()).to_string();

                    if local_name == "calendar-color" {
                        // Check if it's in Apple namespace by looking at the xmlns attribute
                        // iCloud returns: <calendar-color xmlns="http://apple.com/ns/ical/">
                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                            let value = std::str::from_utf8(&attr.value).unwrap_or("");
                            if key == "xmlns" && value == "http://apple.com/ns/ical/" {
                                in_apple_color = true;
                                break;
                            }
                        }
                    }
                }
                Ok(XmlEvent::Text(e)) => {
                    if in_apple_color {
                        color_value = Some(std::str::from_utf8(e.as_ref())?.to_string());
                    }
                }
                Ok(XmlEvent::End(ref e)) => {
                    let name = e.name();
                    let local_name_bytes = name.local_name();
                    let local_name = String::from_utf8_lossy(local_name_bytes.as_ref()).to_string();

                    if local_name == "calendar-color" && in_apple_color {
                        // We found the color, can break early
                        break;
                    }
                }
                Ok(XmlEvent::Eof) => break,
                Err(e) => {
                    debug!("Error parsing Apple color XML: {:?}", e);
                    break;
                }
                _ => {}
            }
            buf.clear();
        }

        Ok(color_value)
    }

    /// Diagnostic function to check what calendar-color properties the server returns
    ///
    /// Makes a custom `PROPFIND` request with both standard `CalDAV` and Apple namespaces
    /// to help debug calendar color issues.
    ///
    /// # Errors
    ///
    /// Returns an error if the PROPFIND request fails
    pub async fn diagnose_calendar_color(
        &self,
        calendar_url: &str,
        base_url: &str,
        username: &str,
        password: &str,
    ) -> Result<()> {
        info!("=== Calendar Color Diagnostic for {} ===", calendar_url);

        // Construct absolute URL from base and relative calendar path
        let absolute_url =
            if calendar_url.starts_with("http://") || calendar_url.starts_with("https://") {
                calendar_url.to_string()
            } else {
                // Remove trailing slash from base_url and leading slash from calendar_url if present
                let base = base_url.trim_end_matches('/');
                let path = calendar_url.trim_start_matches('/');
                format!("{base}/{path}")
            };

        info!("Absolute URL: {}", absolute_url);

        // Build PROPFIND request body with both standard and Apple namespaces
        let propfind_body = r#"<?xml version="1.0" encoding="UTF-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:apple="http://apple.com/ns/ical/">
  <d:prop>
    <d:displayname/>
    <c:calendar-color/>
    <apple:calendar-color/>
  </d:prop>
</d:propfind>"#;

        // Make the request
        let client = reqwest::Client::new();
        let response = client
            .request(reqwest::Method::from_bytes(b"PROPFIND")?, &absolute_url)
            .header("Depth", "0")
            .header("Content-Type", "application/xml; charset=utf-8")
            .basic_auth(username, Some(password))
            .body(propfind_body)
            .send()
            .await?;

        let status = response.status();
        info!("Response status: {}", status);

        let body = response.text().await?;
        info!("Response body:\n{}", body);

        // Parse the XML to extract color properties
        let mut reader = Reader::from_str(&body);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut in_caldav_color = false;
        let mut in_apple_color = false;
        let mut current_text = String::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(XmlEvent::Start(ref e)) => {
                    let name = e.name();
                    let local_name_bytes = name.local_name();
                    let local_name = String::from_utf8_lossy(local_name_bytes.as_ref()).to_string();

                    if local_name == "calendar-color" {
                        // Check namespace
                        if let Some(ns) = name.prefix() {
                            let ns_str = String::from_utf8_lossy(ns.as_ref()).to_string();
                            if ns_str == "c" {
                                in_caldav_color = true;
                                info!("Found CalDAV calendar-color element");
                            } else if ns_str == "apple" {
                                in_apple_color = true;
                                info!("Found Apple calendar-color element");
                            }
                        }
                    }
                }
                Ok(XmlEvent::Text(e)) => {
                    if in_caldav_color || in_apple_color {
                        current_text = std::str::from_utf8(e.as_ref())?.to_string();
                    }
                }
                Ok(XmlEvent::End(ref e)) => {
                    let name = e.name();
                    let local_name_bytes = name.local_name();
                    let local_name = String::from_utf8_lossy(local_name_bytes.as_ref()).to_string();

                    if local_name == "calendar-color" {
                        if in_caldav_color {
                            info!("CalDAV calendar-color value: '{}'", current_text);
                            in_caldav_color = false;
                        } else if in_apple_color {
                            info!("Apple calendar-color value: '{}'", current_text);
                            in_apple_color = false;
                        }
                        current_text.clear();
                    }
                }
                Ok(XmlEvent::Eof) => break,
                Err(e) => {
                    warn!(
                        "Error parsing XML at position {}: {:?}",
                        reader.buffer_position(),
                        e
                    );
                    break;
                }
                _ => {}
            }
            buf.clear();
        }

        info!("=== End Calendar Color Diagnostic ===");
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

        // Convert empty string to None (empty string is just a marker that calendar has been synced)
        let sync_token = sync_token.and_then(|t| if t.is_empty() { None } else { Some(t) });

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
        // The empty string will be converted to None in sync_calendar_incremental
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

                            // Debug logging for Pay Period events
                            if event.summary.contains("Pay") {
                                info!(
                                    "About to expand event '{}' with RRULE: {:?}",
                                    event.summary, event.rrule
                                );
                            }

                            let instances = expand_recurring_event(&event, &config);

                            if event.summary.contains("Pay") {
                                info!(
                                    "Expansion result for '{}': {} instances",
                                    event.summary,
                                    instances.len()
                                );
                            }

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

    // Full integration tests for sync manager are in the integration test suite
}
