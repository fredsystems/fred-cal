// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Represents a calendar event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarEvent {
    /// Unique identifier for the event
    pub uid: String,

    /// Event summary/title
    pub summary: String,

    /// Event description
    pub description: Option<String>,

    /// Event location
    pub location: Option<String>,

    /// Start time of the event
    pub start: DateTime<Utc>,

    /// End time of the event
    pub end: DateTime<Utc>,

    /// Calendar this event belongs to
    pub calendar_name: String,

    /// Calendar URL/path
    pub calendar_url: String,

    /// Calendar color (hex format, e.g., "#FF5733")
    pub calendar_color: Option<String>,

    /// Whether this is an all-day event
    pub all_day: bool,

    /// Recurrence rule (if any)
    pub rrule: Option<String>,

    /// Exception dates (EXDATE) for recurring events - dates to exclude
    pub exdates: Vec<DateTime<Utc>>,

    /// Event status (CONFIRMED, TENTATIVE, CANCELLED)
    pub status: Option<String>,

    /// `ETag` for sync purposes
    pub etag: Option<String>,
}

/// Represents a todo/task
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Todo {
    /// Unique identifier for the todo
    pub uid: String,

    /// Todo summary/title
    pub summary: String,

    /// Todo description
    pub description: Option<String>,

    /// Due date (if any)
    pub due: Option<DateTime<Utc>>,

    /// Start date (if any)
    pub start: Option<DateTime<Utc>>,

    /// Completion date (if completed)
    pub completed: Option<DateTime<Utc>>,

    /// Priority (1-9, 1 being highest)
    pub priority: Option<u8>,

    /// Completion percentage (0-100)
    pub percent_complete: Option<u8>,

    /// Status (NEEDS-ACTION, IN-PROCESS, COMPLETED, CANCELLED)
    pub status: String,

    /// Calendar this todo belongs to
    pub calendar_name: String,

    /// Calendar URL/path
    pub calendar_url: String,

    /// `ETag` for sync purposes
    pub etag: Option<String>,
}

/// Container for all calendar data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarData {
    /// All calendar events
    pub events: Vec<CalendarEvent>,

    /// All todos
    pub todos: Vec<Todo>,

    /// Last sync timestamp
    pub last_sync: DateTime<Utc>,

    /// Sync tokens per calendar for incremental updates
    /// Maps calendar URL to sync token
    #[serde(default)]
    pub sync_tokens: std::collections::HashMap<String, String>,
}

impl CalendarData {
    /// Create new empty calendar data
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            todos: Vec::new(),
            last_sync: Utc::now(),
            sync_tokens: std::collections::HashMap::new(),
        }
    }

    /// Get events for a specific date range
    #[must_use]
    pub fn events_in_range(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<&CalendarEvent> {
        self.events
            .iter()
            .filter(|event| {
                // Event overlaps with the range if:
                // - Event starts before range ends AND
                // - Event ends after range starts
                event.start < end && event.end > start
            })
            .collect()
    }

    /// Get todos due in a specific date range
    #[must_use]
    #[allow(clippy::option_if_let_else)]
    pub fn todos_in_range(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<&Todo> {
        self.todos
            .iter()
            .filter(|todo| {
                if let Some(due) = todo.due {
                    due >= start && due < end
                } else if let Some(start_date) = todo.start {
                    start_date >= start && start_date < end
                } else {
                    // Include todos without dates in all ranges
                    true
                }
            })
            .collect()
    }

    /// Get all incomplete todos
    #[must_use]
    #[allow(dead_code)]
    pub fn incomplete_todos(&self) -> Vec<&Todo> {
        self.todos
            .iter()
            .filter(|todo| todo.status != "COMPLETED" && todo.status != "CANCELLED")
            .collect()
    }
}

impl Default for CalendarData {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_calendar_data_new() {
        let data = CalendarData::new();
        assert!(data.events.is_empty());
        assert!(data.todos.is_empty());
    }

    #[test]
    fn test_events_in_range() {
        let mut data = CalendarData::new();

        let event1 = CalendarEvent {
            uid: "1".to_string(),
            summary: "Event 1".to_string(),
            description: None,
            location: None,
            start: Utc
                .with_ymd_and_hms(2026, 1, 5, 10, 0, 0)
                .single()
                .expect("valid datetime"),
            end: Utc
                .with_ymd_and_hms(2026, 1, 5, 11, 0, 0)
                .single()
                .expect("valid datetime"),
            calendar_name: "Test".to_string(),
            calendar_url: "/test".to_string(),
            calendar_color: None,
            all_day: false,
            rrule: None,
            exdates: Vec::new(),
            status: None,
            etag: None,
        };

        let event2 = CalendarEvent {
            uid: "2".to_string(),
            summary: "Event 2".to_string(),
            description: None,
            location: None,
            start: Utc
                .with_ymd_and_hms(2026, 1, 10, 14, 0, 0)
                .single()
                .expect("valid datetime"),
            end: Utc
                .with_ymd_and_hms(2026, 1, 10, 15, 0, 0)
                .single()
                .expect("valid datetime"),
            calendar_name: "Test".to_string(),
            calendar_url: "/test".to_string(),
            calendar_color: None,
            all_day: false,
            rrule: None,
            exdates: Vec::new(),
            status: None,
            etag: None,
        };

        data.events.push(event1);
        data.events.push(event2);

        let start = Utc
            .with_ymd_and_hms(2026, 1, 5, 0, 0, 0)
            .single()
            .expect("valid datetime");
        let end = Utc
            .with_ymd_and_hms(2026, 1, 6, 0, 0, 0)
            .single()
            .expect("valid datetime");

        let filtered_events = data.events_in_range(start, end);
        assert_eq!(filtered_events.len(), 1);
        assert_eq!(filtered_events[0].uid, "1");
    }

    #[test]
    fn test_todos_in_range() {
        let mut data = CalendarData::new();

        let todo1 = Todo {
            uid: "1".to_string(),
            summary: "Todo 1".to_string(),
            description: None,
            due: Some(
                Utc.with_ymd_and_hms(2026, 1, 5, 12, 0, 0)
                    .single()
                    .expect("valid datetime"),
            ),
            start: None,
            completed: None,
            priority: None,
            percent_complete: None,
            status: "NEEDS-ACTION".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: None,
        };

        let todo2 = Todo {
            uid: "2".to_string(),
            summary: "Todo 2".to_string(),
            description: None,
            due: Some(
                Utc.with_ymd_and_hms(2026, 1, 10, 12, 0, 0)
                    .single()
                    .expect("valid datetime"),
            ),
            start: None,
            completed: None,
            priority: None,
            percent_complete: None,
            status: "NEEDS-ACTION".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: None,
        };

        data.todos.push(todo1);
        data.todos.push(todo2);

        let start = Utc
            .with_ymd_and_hms(2026, 1, 5, 0, 0, 0)
            .single()
            .expect("valid datetime");
        let end = Utc
            .with_ymd_and_hms(2026, 1, 6, 0, 0, 0)
            .single()
            .expect("valid datetime");

        let filtered_todos = data.todos_in_range(start, end);
        assert_eq!(filtered_todos.len(), 1);
        assert_eq!(filtered_todos[0].uid, "1");
    }

    #[test]
    fn test_incomplete_todos() {
        let mut data = CalendarData::new();

        let todo1 = Todo {
            uid: "1".to_string(),
            summary: "Active Todo".to_string(),
            description: None,
            due: None,
            start: None,
            completed: None,
            priority: None,
            percent_complete: None,
            status: "NEEDS-ACTION".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: None,
        };

        let todo2 = Todo {
            uid: "2".to_string(),
            summary: "Completed Todo".to_string(),
            description: None,
            due: None,
            start: None,
            completed: Some(Utc::now()),
            priority: None,
            percent_complete: Some(100),
            status: "COMPLETED".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: None,
        };

        data.todos.push(todo1);
        data.todos.push(todo2);

        let incomplete = data.incomplete_todos();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].uid, "1");
    }

    #[test]
    fn test_all_day_event_overlap() {
        let mut data = CalendarData::new();

        // All-day event from Jan 2-4 (stored as Jan 2 00:00 to Jan 4 00:00 UTC)
        let all_day_event = CalendarEvent {
            uid: "all-day-1".to_string(),
            summary: "Multi-day Event".to_string(),
            description: None,
            location: None,
            start: Utc
                .with_ymd_and_hms(2026, 1, 2, 0, 0, 0)
                .single()
                .expect("valid datetime"),
            end: Utc
                .with_ymd_and_hms(2026, 1, 4, 0, 0, 0)
                .single()
                .expect("valid datetime"),
            calendar_name: "Test".to_string(),
            calendar_url: "/test".to_string(),
            calendar_color: None,
            all_day: true,
            rrule: None,
            exdates: Vec::new(),
            status: None,
            etag: None,
        };

        data.events.push(all_day_event);

        // Query for Jan 3 in Mountain Time (UTC-7): 07:00 UTC Jan 3 to 07:00 UTC Jan 4
        let start = Utc
            .with_ymd_and_hms(2026, 1, 3, 7, 0, 0)
            .single()
            .expect("valid datetime");
        let end = Utc
            .with_ymd_and_hms(2026, 1, 4, 7, 0, 0)
            .single()
            .expect("valid datetime");

        let filtered_events = data.events_in_range(start, end);

        // Should find the all-day event even though it neither starts nor ends on Jan 3
        assert_eq!(filtered_events.len(), 1);
        assert_eq!(filtered_events[0].uid, "all-day-1");
        assert!(filtered_events[0].all_day);
    }

    #[test]
    fn test_all_day_event_edge_cases() {
        let mut data = CalendarData::new();

        // Single-day all-day event on Jan 3
        let single_day = CalendarEvent {
            uid: "single-day".to_string(),
            summary: "Single Day".to_string(),
            description: None,
            location: None,
            start: Utc
                .with_ymd_and_hms(2026, 1, 3, 0, 0, 0)
                .single()
                .expect("valid datetime"),
            end: Utc
                .with_ymd_and_hms(2026, 1, 4, 0, 0, 0)
                .single()
                .expect("valid datetime"),
            calendar_name: "Test".to_string(),
            calendar_url: "/test".to_string(),
            calendar_color: None,
            all_day: true,
            rrule: None,
            exdates: Vec::new(),
            status: None,
            etag: None,
        };

        // Event that ends at midnight on query start (should not overlap)
        let ends_at_midnight = CalendarEvent {
            uid: "ends-midnight".to_string(),
            summary: "Ends at Midnight".to_string(),
            description: None,
            location: None,
            start: Utc
                .with_ymd_and_hms(2026, 1, 2, 0, 0, 0)
                .single()
                .expect("valid datetime"),
            end: Utc
                .with_ymd_and_hms(2026, 1, 3, 0, 0, 0)
                .single()
                .expect("valid datetime"),
            calendar_name: "Test".to_string(),
            calendar_url: "/test".to_string(),
            calendar_color: None,
            all_day: true,
            rrule: None,
            exdates: Vec::new(),
            status: None,
            etag: None,
        };

        data.events.push(single_day);
        data.events.push(ends_at_midnight);

        // Query for Jan 3 Mountain Time
        let start = Utc
            .with_ymd_and_hms(2026, 1, 3, 7, 0, 0)
            .single()
            .expect("valid datetime");
        let end = Utc
            .with_ymd_and_hms(2026, 1, 4, 7, 0, 0)
            .single()
            .expect("valid datetime");

        let filtered_events = data.events_in_range(start, end);

        // Should only find the single-day event
        assert_eq!(filtered_events.len(), 1);
        assert_eq!(filtered_events[0].uid, "single-day");
    }

    #[test]
    fn test_todos_with_start_date_only() {
        let mut data = CalendarData::new();

        // Todo with start date but no due date
        let todo = Todo {
            uid: "start-only".to_string(),
            summary: "Start Date Only".to_string(),
            description: None,
            due: None,
            start: Some(
                Utc.with_ymd_and_hms(2026, 1, 5, 12, 0, 0)
                    .single()
                    .expect("valid datetime"),
            ),
            completed: None,
            priority: None,
            percent_complete: None,
            status: "IN-PROCESS".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: None,
        };

        data.todos.push(todo);

        let start = Utc
            .with_ymd_and_hms(2026, 1, 5, 0, 0, 0)
            .single()
            .expect("valid datetime");
        let end = Utc
            .with_ymd_and_hms(2026, 1, 6, 0, 0, 0)
            .single()
            .expect("valid datetime");

        let filtered_todos = data.todos_in_range(start, end);
        assert_eq!(filtered_todos.len(), 1);
        assert_eq!(filtered_todos[0].uid, "start-only");
    }

    #[test]
    fn test_todos_without_dates() {
        let mut data = CalendarData::new();

        // Todo with neither due nor start date - should appear in all ranges
        let todo = Todo {
            uid: "no-dates".to_string(),
            summary: "No Dates".to_string(),
            description: Some("A task without dates".to_string()),
            due: None,
            start: None,
            completed: None,
            priority: Some(1),
            percent_complete: Some(0),
            status: "NEEDS-ACTION".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: Some("etag123".to_string()),
        };

        data.todos.push(todo);

        // Query any date range
        let start = Utc
            .with_ymd_and_hms(2026, 1, 5, 0, 0, 0)
            .single()
            .expect("valid datetime");
        let end = Utc
            .with_ymd_and_hms(2026, 1, 6, 0, 0, 0)
            .single()
            .expect("valid datetime");

        let filtered_todos = data.todos_in_range(start, end);
        assert_eq!(filtered_todos.len(), 1);
        assert_eq!(filtered_todos[0].uid, "no-dates");
    }

    #[test]
    fn test_calendar_event_partial_eq() {
        let event1 = CalendarEvent {
            uid: "test".to_string(),
            summary: "Test Event".to_string(),
            description: Some("Description".to_string()),
            location: Some("Location".to_string()),
            start: Utc
                .with_ymd_and_hms(2026, 1, 5, 10, 0, 0)
                .single()
                .expect("valid datetime"),
            end: Utc
                .with_ymd_and_hms(2026, 1, 5, 11, 0, 0)
                .single()
                .expect("valid datetime"),
            calendar_name: "Calendar".to_string(),
            calendar_url: "/calendar".to_string(),
            calendar_color: Some("#FF0000".to_string()),
            all_day: false,
            rrule: None,
            exdates: Vec::new(),
            status: Some("Confirmed".to_string()),
            etag: Some("etag123".to_string()),
        };

        let event2 = event1.clone();
        assert_eq!(event1, event2);

        let mut event3 = event1.clone();
        event3.uid = "different".to_string();
        assert_ne!(event1, event3);
    }

    #[test]
    fn test_todo_partial_eq() {
        let todo1 = Todo {
            uid: "test".to_string(),
            summary: "Test Todo".to_string(),
            description: Some("Description".to_string()),
            due: Some(
                Utc.with_ymd_and_hms(2026, 1, 10, 12, 0, 0)
                    .single()
                    .expect("valid datetime"),
            ),
            start: Some(
                Utc.with_ymd_and_hms(2026, 1, 5, 9, 0, 0)
                    .single()
                    .expect("valid datetime"),
            ),
            completed: Some(Utc::now()),
            priority: Some(5),
            percent_complete: Some(75),
            status: "IN-PROCESS".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: Some("etag1".to_string()),
        };

        let todo2 = todo1.clone();
        assert_eq!(todo1, todo2);

        let mut todo3 = todo1.clone();
        todo3.summary = "Different".to_string();
        assert_ne!(todo1, todo3);
    }

    #[test]
    fn test_calendar_data_default() {
        let data = CalendarData::default();
        assert!(data.events.is_empty());
        assert!(data.todos.is_empty());
        assert!(data.sync_tokens.is_empty());
    }

    #[test]
    fn test_calendar_data_clone() {
        let mut data = CalendarData::new();
        data.events.push(CalendarEvent {
            uid: "1".to_string(),
            summary: "Event".to_string(),
            description: None,
            location: None,
            start: Utc::now(),
            end: Utc::now(),
            calendar_name: "Test".to_string(),
            calendar_url: "/test".to_string(),
            calendar_color: None,
            all_day: false,
            rrule: None,
            exdates: Vec::new(),
            status: None,
            etag: None,
        });
        data.sync_tokens
            .insert("calendar1".to_string(), "token123".to_string());

        let cloned = data.clone();
        assert_eq!(cloned.events.len(), data.events.len());
        assert_eq!(cloned.sync_tokens.len(), data.sync_tokens.len());
        assert_eq!(
            cloned.sync_tokens.get("calendar1"),
            Some(&"token123".to_string())
        );
    }

    #[test]
    fn test_calendar_event_debug() {
        let event = CalendarEvent {
            uid: "test".to_string(),
            summary: "Test Event".to_string(),
            description: None,
            location: None,
            start: Utc
                .with_ymd_and_hms(2026, 1, 5, 10, 0, 0)
                .single()
                .expect("valid datetime"),
            end: Utc
                .with_ymd_and_hms(2026, 1, 5, 11, 0, 0)
                .single()
                .expect("valid datetime"),
            calendar_name: "Test".to_string(),
            calendar_url: "/test".to_string(),
            calendar_color: None,
            all_day: false,
            rrule: None,
            exdates: Vec::new(),
            status: None,
            etag: None,
        };

        let debug_str = format!("{event:?}");
        assert!(debug_str.contains("CalendarEvent"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_todo_debug() {
        let todo = Todo {
            uid: "test".to_string(),
            summary: "Test Todo".to_string(),
            description: None,
            due: None,
            start: None,
            completed: None,
            priority: None,
            percent_complete: None,
            status: "NEEDS-ACTION".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: None,
        };

        let debug_str = format!("{todo:?}");
        assert!(debug_str.contains("Todo"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_calendar_data_debug() {
        let data = CalendarData::new();
        let debug_str = format!("{data:?}");
        assert!(debug_str.contains("CalendarData"));
    }

    #[test]
    fn test_incomplete_todos_with_cancelled() {
        let mut data = CalendarData::new();

        let todo1 = Todo {
            uid: "1".to_string(),
            summary: "Active".to_string(),
            description: None,
            due: None,
            start: None,
            completed: None,
            priority: None,
            percent_complete: None,
            status: "NEEDS-ACTION".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: None,
        };

        let todo2 = Todo {
            uid: "2".to_string(),
            summary: "Cancelled".to_string(),
            description: None,
            due: None,
            start: None,
            completed: None,
            priority: None,
            percent_complete: None,
            status: "CANCELLED".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: None,
        };

        data.todos.push(todo1);
        data.todos.push(todo2);

        let incomplete = data.incomplete_todos();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].status, "NEEDS-ACTION");
    }

    #[test]
    fn test_sync_tokens() {
        let mut data = CalendarData::new();

        data.sync_tokens
            .insert("/calendar/1".to_string(), "sync-token-1".to_string());
        data.sync_tokens
            .insert("/calendar/2".to_string(), "sync-token-2".to_string());

        assert_eq!(data.sync_tokens.len(), 2);
        assert_eq!(
            data.sync_tokens.get("/calendar/1"),
            Some(&"sync-token-1".to_string())
        );
        assert_eq!(
            data.sync_tokens.get("/calendar/2"),
            Some(&"sync-token-2".to_string())
        );
    }

    #[test]
    fn test_events_in_range_boundary_conditions() {
        let mut data = CalendarData::new();

        // Event that starts exactly at range start
        let event1 = CalendarEvent {
            uid: "1".to_string(),
            summary: "Starts at boundary".to_string(),
            description: None,
            location: None,
            start: Utc
                .with_ymd_and_hms(2026, 1, 5, 0, 0, 0)
                .single()
                .expect("valid datetime"),
            end: Utc
                .with_ymd_and_hms(2026, 1, 5, 1, 0, 0)
                .single()
                .expect("valid datetime"),
            calendar_name: "Test".to_string(),
            calendar_url: "/test".to_string(),
            calendar_color: None,
            all_day: false,
            rrule: None,
            exdates: Vec::new(),
            status: None,
            etag: None,
        };

        // Event that ends exactly at range end
        let event2 = CalendarEvent {
            uid: "2".to_string(),
            summary: "Ends at boundary".to_string(),
            description: None,
            location: None,
            start: Utc
                .with_ymd_and_hms(2026, 1, 5, 23, 0, 0)
                .single()
                .expect("valid datetime"),
            end: Utc
                .with_ymd_and_hms(2026, 1, 6, 0, 0, 0)
                .single()
                .expect("valid datetime"),
            calendar_name: "Test".to_string(),
            calendar_url: "/test".to_string(),
            calendar_color: None,
            all_day: false,
            rrule: None,
            exdates: Vec::new(),
            status: None,
            etag: None,
        };

        data.events.push(event1);
        data.events.push(event2);

        let start = Utc
            .with_ymd_and_hms(2026, 1, 5, 0, 0, 0)
            .single()
            .expect("valid datetime");
        let end = Utc
            .with_ymd_and_hms(2026, 1, 6, 0, 0, 0)
            .single()
            .expect("valid datetime");

        let filtered_events = data.events_in_range(start, end);
        assert_eq!(filtered_events.len(), 2);
    }

    #[test]
    fn test_todos_in_range_boundary_conditions() {
        let mut data = CalendarData::new();

        // Todo due exactly at range start
        let todo1 = Todo {
            uid: "1".to_string(),
            summary: "Due at start".to_string(),
            description: None,
            due: Some(
                Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0)
                    .single()
                    .expect("valid datetime"),
            ),
            start: None,
            completed: None,
            priority: None,
            percent_complete: None,
            status: "NEEDS-ACTION".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: None,
        };

        // Todo due exactly at range end (should not be included)
        let todo2 = Todo {
            uid: "2".to_string(),
            summary: "Due at end".to_string(),
            description: None,
            due: Some(
                Utc.with_ymd_and_hms(2026, 1, 6, 0, 0, 0)
                    .single()
                    .expect("valid datetime"),
            ),
            start: None,
            completed: None,
            priority: None,
            percent_complete: None,
            status: "NEEDS-ACTION".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: None,
        };

        data.todos.push(todo1);
        data.todos.push(todo2);

        let start = Utc
            .with_ymd_and_hms(2026, 1, 5, 0, 0, 0)
            .single()
            .expect("valid datetime");
        let end = Utc
            .with_ymd_and_hms(2026, 1, 6, 0, 0, 0)
            .single()
            .expect("valid datetime");

        let filtered_todos = data.todos_in_range(start, end);
        // Only todo1 should be included (due < end, not due == end)
        assert_eq!(filtered_todos.len(), 1);
        assert_eq!(filtered_todos[0].uid, "1");
    }
}
