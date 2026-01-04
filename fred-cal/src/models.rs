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

    /// Whether this is an all-day event
    pub all_day: bool,

    /// Recurrence rule (if any)
    pub rrule: Option<String>,

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
            all_day: false,
            rrule: None,
            status: None,
            etag: None,
        };

        let event2 = CalendarEvent {
            uid: "2".to_string(),
            summary: "Event 2".to_string(),
            description: None,
            location: None,
            start: Utc
                .with_ymd_and_hms(2026, 1, 10, 10, 0, 0)
                .single()
                .expect("valid datetime"),
            end: Utc
                .with_ymd_and_hms(2026, 1, 10, 11, 0, 0)
                .single()
                .expect("valid datetime"),
            calendar_name: "Test".to_string(),
            calendar_url: "/test".to_string(),
            all_day: false,
            rrule: None,
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
            all_day: true,
            rrule: None,
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
            all_day: true,
            rrule: None,
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
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid datetime"),
            end: Utc
                .with_ymd_and_hms(2026, 1, 3, 7, 0, 0)
                .single()
                .expect("valid datetime"),
            calendar_name: "Test".to_string(),
            calendar_url: "/test".to_string(),
            all_day: true,
            rrule: None,
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
}
