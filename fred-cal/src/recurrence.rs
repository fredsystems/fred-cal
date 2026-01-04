// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::models::CalendarEvent;
use chrono::{DateTime, Utc};
use rrule::RRuleSet;

/// Configuration for recurrence expansion
#[derive(Debug, Clone)]
pub struct RecurrenceConfig {
    /// How many days forward from now to expand recurring events
    pub expand_forward_days: i64,
    /// How many days backward from now to expand recurring events
    pub expand_backward_days: i64,
}

impl Default for RecurrenceConfig {
    fn default() -> Self {
        Self {
            expand_forward_days: 730,  // 2 years
            expand_backward_days: 365, // 1 year
        }
    }
}

/// Expand a recurring event into individual occurrences
///
/// Takes an event with an RRULE and generates individual event instances
/// for each occurrence within the expansion window.
///
/// # Arguments
///
/// * `event` - The master recurring event (with rrule field)
/// * `config` - Configuration for expansion window
///
/// # Returns
///
/// A vector of event instances, one for each occurrence. If the event has no
/// RRULE or if RRULE parsing fails, returns a vector containing only the
/// original event.
pub fn expand_recurring_event(
    event: &CalendarEvent,
    config: &RecurrenceConfig,
) -> Vec<CalendarEvent> {
    // If no RRULE, return the event as-is
    let Some(rrule_str) = &event.rrule else {
        return vec![event.clone()];
    };

    // Parse the RRULE
    let rrule_result = parse_rrule(rrule_str, event.start);
    let rrule_set = match rrule_result {
        Ok(set) => set,
        Err(e) => {
            warn!(
                "Failed to parse RRULE for event '{}': {}. Using original event only.",
                event.summary, e
            );
            return vec![event.clone()];
        }
    };

    // Calculate expansion window
    let now = Utc::now();
    let window_start = now - chrono::Duration::days(config.expand_backward_days);
    let window_end = now + chrono::Duration::days(config.expand_forward_days);

    debug!(
        "Expanding recurring event '{}' from {} to {}",
        event.summary, window_start, window_end
    );

    // Generate occurrences within the window
    // We need to limit the iteration to avoid generating thousands of occurrences
    // for long-running recurring events
    let duration = event.end - event.start;
    let mut instances = Vec::new();

    for occurrence in &rrule_set {
        // Convert to UTC DateTime
        // The rrule crate returns DateTime<rrule::Tz>, we need DateTime<Utc>
        let occurrence_start = occurrence.with_timezone(&Utc);

        // Stop if we're past the window end
        if occurrence_start > window_end {
            break;
        }

        // Skip if before window start
        if occurrence_start < window_start {
            continue;
        }

        // Create event instance for this occurrence
        let occurrence_end = occurrence_start + duration;

        let mut instance = event.clone();
        instance.start = occurrence_start;
        instance.end = occurrence_end;

        // Keep the RRULE in the instance so we know it's part of a recurring series
        // but we could also clear it if we want each instance to be independent

        instances.push(instance);
    }

    // If no instances were generated in the window, include the original
    // This handles edge cases where the RRULE might have already ended
    if instances.is_empty() {
        debug!(
            "No occurrences in window for '{}', including original event",
            event.summary
        );
        instances.push(event.clone());
    }

    instances
}

/// Parse an RRULE string into an `RRuleSet`
///
/// # Arguments
///
/// * `rrule_str` - The RRULE string (e.g., "FREQ=WEEKLY;INTERVAL=2")
/// * `dtstart` - The start datetime of the original event
///
/// # Returns
///
/// An `RRuleSet` that can generate occurrences
fn parse_rrule(rrule_str: &str, dtstart: DateTime<Utc>) -> Result<RRuleSet, String> {
    // Normalize UNTIL dates to UTC format
    // Some calendars provide UNTIL in local/floating format (YYYYMMDD or YYYYMMDDTHHMMSS)
    // but the rrule crate requires UNTIL to match DTSTART timezone (UTC in our case)
    let normalized_rrule = normalize_until_to_utc(rrule_str);

    // Build the complete RRULE with DTSTART
    // The rrule crate expects a full iCalendar RRULE format
    let rrule_with_dtstart = format!(
        "DTSTART:{}\nRRULE:{}",
        dtstart.format("%Y%m%dT%H%M%SZ"),
        normalized_rrule
    );

    debug!("Parsing RRULE: {}", rrule_with_dtstart);

    // Parse the RRULE
    rrule_with_dtstart
        .parse::<RRuleSet>()
        .map_err(|e| format!("RRULE parse error: {e}"))
}

/// Normalize UNTIL dates in RRULE to UTC format
///
/// Converts UNTIL=YYYYMMDD or UNTIL=YYYYMMDDTHHMMSS to UNTIL=YYYYMMDDTHHMMSSZ
fn normalize_until_to_utc(rrule_str: &str) -> String {
    // Look for UNTIL= parameter
    if !rrule_str.contains("UNTIL=") {
        return rrule_str.to_string();
    }

    // Split by semicolon to process parameters
    let parts: Vec<&str> = rrule_str.split(';').collect();
    let normalized_parts: Vec<String> = parts
        .iter()
        .map(|part| {
            part.strip_prefix("UNTIL=").map_or_else(
                || part.to_string(),
                |until_value| {
                    // Check if it already ends with Z (UTC)
                    if until_value.ends_with('Z') {
                        part.to_string()
                    } else if until_value.len() == 8
                        && until_value.chars().all(|c| c.is_ascii_digit())
                    {
                        // If it's just a date (8 digits), convert to datetime at midnight UTC
                        format!("UNTIL={until_value}T000000Z")
                    } else if until_value.len() == 15
                        && until_value.chars().all(|c| c.is_ascii_digit() || c == 'T')
                    {
                        // If it's a datetime without Z, add Z
                        format!("UNTIL={until_value}Z")
                    } else {
                        // Otherwise return as-is
                        part.to_string()
                    }
                },
            )
        })
        .collect();

    normalized_parts.join(";")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn create_test_event(
        summary: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        rrule: Option<String>,
    ) -> CalendarEvent {
        CalendarEvent {
            uid: "test-uid".to_string(),
            summary: summary.to_string(),
            description: None,
            location: None,
            start,
            end,
            calendar_name: "Test".to_string(),
            calendar_url: "/test".to_string(),
            calendar_color: None,
            all_day: false,
            rrule,
            status: None,
            etag: None,
        }
    }

    #[test]
    fn test_expand_no_rrule() {
        let start = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 1, 1, 11, 0, 0).unwrap();
        let event = create_test_event("Test", start, end, None);

        let config = RecurrenceConfig::default();
        let instances = expand_recurring_event(&event, &config);

        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].start, start);
    }

    #[test]
    fn test_expand_daily_rrule() {
        let start = Utc::now().date_naive().and_hms_opt(10, 0, 0).unwrap();
        let start = Utc.from_utc_datetime(&start);
        let end = start + chrono::Duration::hours(1);

        let event = create_test_event("Daily", start, end, Some("FREQ=DAILY;COUNT=5".to_string()));

        let config = RecurrenceConfig {
            expand_forward_days: 30,
            expand_backward_days: 1,
        };
        let instances = expand_recurring_event(&event, &config);

        // Should get 5 instances (COUNT=5)
        assert_eq!(instances.len(), 5);

        // Verify they're on consecutive days
        for (i, instance) in instances.iter().enumerate() {
            let expected_start = start + chrono::Duration::days(i64::try_from(i).unwrap());
            assert_eq!(instance.start, expected_start);
        }
    }

    #[test]
    fn test_expand_weekly_rrule() {
        let start = Utc::now().date_naive().and_hms_opt(10, 0, 0).unwrap();
        let start = Utc.from_utc_datetime(&start);
        let end = start + chrono::Duration::hours(1);

        let event = create_test_event(
            "Weekly",
            start,
            end,
            Some("FREQ=WEEKLY;COUNT=4".to_string()),
        );

        let config = RecurrenceConfig {
            expand_forward_days: 60,
            expand_backward_days: 1,
        };
        let instances = expand_recurring_event(&event, &config);

        // Should get 4 instances
        assert_eq!(instances.len(), 4);

        // Verify they're 7 days apart
        for window in instances.windows(2) {
            let diff = window[1].start - window[0].start;
            assert_eq!(diff.num_days(), 7);
        }
    }

    #[test]
    fn test_expand_with_interval() {
        let start = Utc::now().date_naive().and_hms_opt(10, 0, 0).unwrap();
        let start = Utc.from_utc_datetime(&start);
        let end = start + chrono::Duration::hours(1);

        // Every 2 weeks
        let event = create_test_event(
            "Bi-weekly",
            start,
            end,
            Some("FREQ=WEEKLY;INTERVAL=2;COUNT=3".to_string()),
        );

        let config = RecurrenceConfig {
            expand_forward_days: 60,
            expand_backward_days: 1,
        };
        let instances = expand_recurring_event(&event, &config);

        // Should get 3 instances
        assert_eq!(instances.len(), 3);

        // Verify they're 14 days apart
        for window in instances.windows(2) {
            let diff = window[1].start - window[0].start;
            assert_eq!(diff.num_days(), 14);
        }
    }

    #[test]
    fn test_expand_invalid_rrule() {
        let start = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 1, 1, 11, 0, 0).unwrap();

        let event = create_test_event("Invalid", start, end, Some("INVALID_RRULE".to_string()));

        let config = RecurrenceConfig::default();
        let instances = expand_recurring_event(&event, &config);

        // Should fall back to original event
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].start, start);
    }

    #[test]
    fn test_expansion_window() {
        // Event starting soon with daily recurrence
        let start = Utc::now().date_naive().and_hms_opt(10, 0, 0).unwrap();
        let start = Utc.from_utc_datetime(&start);
        let end = start + chrono::Duration::hours(1);

        // Create event with many occurrences
        let event = create_test_event(
            "Daily",
            start,
            end,
            Some("FREQ=DAILY;COUNT=100".to_string()),
        );

        let config = RecurrenceConfig {
            expand_forward_days: 30,
            expand_backward_days: 30,
        };
        let instances = expand_recurring_event(&event, &config);

        // Should only get instances within the 60-day window around now
        for instance in &instances {
            let now = Utc::now();
            let diff = (instance.start - now).num_days().abs();
            assert!(
                diff <= 30,
                "Instance at {} is {} days from now, expected <= 30",
                instance.start,
                diff
            );
        }

        // Should have gotten some instances but not all 100
        assert!(!instances.is_empty());
        assert!(instances.len() < 100);
    }

    #[test]
    fn test_event_duration_preserved() {
        let start = Utc::now().date_naive().and_hms_opt(10, 0, 0).unwrap();
        let start = Utc.from_utc_datetime(&start);
        let end = start + chrono::Duration::hours(2); // 2-hour event

        let event = create_test_event(
            "Long Event",
            start,
            end,
            Some("FREQ=DAILY;COUNT=3".to_string()),
        );

        let config = RecurrenceConfig {
            expand_forward_days: 30,
            expand_backward_days: 1,
        };
        let instances = expand_recurring_event(&event, &config);

        // All instances should have the same 2-hour duration
        for instance in &instances {
            let duration = instance.end - instance.start;
            assert_eq!(duration.num_hours(), 2);
        }
    }

    #[test]
    fn test_pay_period_rrule() {
        // Test the exact RRULE from Pay Period 3
        let start = Utc.with_ymd_and_hms(2014, 1, 12, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2014, 1, 26, 0, 0, 0).unwrap();

        let event = create_test_event(
            "Pay Period 3",
            start,
            end,
            Some("FREQ=WEEKLY;INTERVAL=52".to_string()),
        );

        let config = RecurrenceConfig {
            expand_forward_days: 730,  // 2 years
            expand_backward_days: 365, // 1 year
        };
        let instances = expand_recurring_event(&event, &config);

        // Should get instances in the expansion window (not the 2014 original)
        // The event repeats every 52 weeks from 2014, so we should see 2025, 2026, 2027, 2028
        assert!(!instances.is_empty(), "Expected some instances");

        // Verify instances are within the window
        let now = Utc::now();
        let window_start = now - chrono::Duration::days(365);
        let window_end = now + chrono::Duration::days(730);

        for instance in &instances {
            assert!(
                instance.start >= window_start,
                "Instance {} is before window start {}",
                instance.start,
                window_start
            );
            assert!(
                instance.start <= window_end,
                "Instance {} is after window end {}",
                instance.start,
                window_end
            );
        }

        // Verify they're 52 weeks apart
        if instances.len() > 1 {
            for window in instances.windows(2) {
                let diff = window[1].start - window[0].start;
                assert_eq!(
                    diff.num_days(),
                    364,
                    "Instances should be 364 days (52 weeks) apart"
                );
            }
        }
    }

    #[test]
    fn test_normalize_until_to_utc() {
        // Test date-only UNTIL
        let rrule = "FREQ=WEEKLY;UNTIL=20140315;INTERVAL=52";
        let normalized = normalize_until_to_utc(rrule);
        assert_eq!(normalized, "FREQ=WEEKLY;UNTIL=20140315T000000Z;INTERVAL=52");

        // Test datetime UNTIL without Z
        let rrule = "FREQ=DAILY;UNTIL=20140315T120000;COUNT=10";
        let normalized = normalize_until_to_utc(rrule);
        assert_eq!(normalized, "FREQ=DAILY;UNTIL=20140315T120000Z;COUNT=10");

        // Test already has Z
        let rrule = "FREQ=DAILY;UNTIL=20140315T120000Z";
        let normalized = normalize_until_to_utc(rrule);
        assert_eq!(normalized, "FREQ=DAILY;UNTIL=20140315T120000Z");

        // Test no UNTIL
        let rrule = "FREQ=WEEKLY;INTERVAL=2";
        let normalized = normalize_until_to_utc(rrule);
        assert_eq!(normalized, "FREQ=WEEKLY;INTERVAL=2");
    }

    #[test]
    fn test_rrule_with_until() {
        // Test event with UNTIL that needs normalization
        let start = Utc.with_ymd_and_hms(2014, 1, 1, 10, 0, 0).unwrap();
        let end = start + chrono::Duration::hours(1);

        // This RRULE format comes from some calendars (date-only UNTIL)
        let event = create_test_event(
            "Event with UNTIL",
            start,
            end,
            Some("FREQ=DAILY;UNTIL=20140110".to_string()),
        );

        let config = RecurrenceConfig::default();
        let instances = expand_recurring_event(&event, &config);

        // Should successfully parse and expand (won't error out)
        // The event is from 2014, so no instances in our window, but original included
        assert_eq!(instances.len(), 1);
    }

    #[test]
    fn test_recurrence_config_default() {
        let config = RecurrenceConfig::default();
        assert_eq!(config.expand_forward_days, 730);
        assert_eq!(config.expand_backward_days, 365);
    }

    #[test]
    fn test_recurrence_config_clone() {
        let config = RecurrenceConfig {
            expand_forward_days: 100,
            expand_backward_days: 50,
        };
        #[allow(clippy::redundant_clone)]
        let cloned = config.clone();
        assert_eq!(cloned.expand_forward_days, 100);
        assert_eq!(cloned.expand_backward_days, 50);
    }

    #[test]
    fn test_recurrence_config_debug() {
        let config = RecurrenceConfig::default();
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("RecurrenceConfig"));
        assert!(debug_str.contains("730"));
        assert!(debug_str.contains("365"));
    }

    #[test]
    fn test_normalize_until_invalid_format() {
        // Test UNTIL with invalid format (not 8 or 15 chars, or has invalid chars)
        let rrule = "FREQ=DAILY;UNTIL=invalid;COUNT=5";
        let normalized = normalize_until_to_utc(rrule);
        // Should return as-is when format doesn't match expectations
        assert_eq!(normalized, "FREQ=DAILY;UNTIL=invalid;COUNT=5");
    }

    #[test]
    fn test_normalize_until_with_multiple_params() {
        // Test RRULE with multiple parameters including UNTIL
        let rrule = "FREQ=WEEKLY;BYDAY=MO,WE,FR;UNTIL=20260315;INTERVAL=1";
        let normalized = normalize_until_to_utc(rrule);
        assert_eq!(
            normalized,
            "FREQ=WEEKLY;BYDAY=MO,WE,FR;UNTIL=20260315T000000Z;INTERVAL=1"
        );
    }

    #[test]
    fn test_expand_with_custom_config() {
        let start = Utc::now().date_naive().and_hms_opt(14, 30, 0).unwrap();
        let start = Utc.from_utc_datetime(&start);
        let end = start + chrono::Duration::minutes(45);

        let event = create_test_event(
            "Custom window",
            start,
            end,
            Some("FREQ=DAILY;COUNT=20".to_string()),
        );

        // Small window
        let config = RecurrenceConfig {
            expand_forward_days: 5,
            expand_backward_days: 2,
        };
        let instances = expand_recurring_event(&event, &config);

        // Should only get instances within 7-day window
        assert!(instances.len() <= 7);
    }

    #[test]
    fn test_expand_event_past_window() {
        // Event from the past that's already ended
        let start = Utc.with_ymd_and_hms(2000, 1, 1, 10, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2000, 1, 1, 11, 0, 0).unwrap();

        let event = create_test_event(
            "Past event",
            start,
            end,
            Some("FREQ=DAILY;UNTIL=20000105".to_string()),
        );

        let config = RecurrenceConfig {
            expand_forward_days: 30,
            expand_backward_days: 30,
        };
        let instances = expand_recurring_event(&event, &config);

        // No instances in window, should include original
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].start, start);
    }

    #[test]
    fn test_expand_monthly_rrule() {
        let start = Utc::now().date_naive().and_hms_opt(9, 0, 0).unwrap();
        let start = Utc.from_utc_datetime(&start);
        let end = start + chrono::Duration::hours(1);

        let event = create_test_event(
            "Monthly",
            start,
            end,
            Some("FREQ=MONTHLY;COUNT=3".to_string()),
        );

        let config = RecurrenceConfig {
            expand_forward_days: 120,
            expand_backward_days: 1,
        };
        let instances = expand_recurring_event(&event, &config);

        // Should get 3 instances
        assert_eq!(instances.len(), 3);
    }

    #[test]
    fn test_expand_with_byday() {
        let start = Utc::now().date_naive().and_hms_opt(15, 0, 0).unwrap();
        let start = Utc.from_utc_datetime(&start);
        let end = start + chrono::Duration::hours(1);

        // Every Monday and Wednesday
        let event = create_test_event(
            "MW Meeting",
            start,
            end,
            Some("FREQ=WEEKLY;BYDAY=MO,WE;COUNT=6".to_string()),
        );

        let config = RecurrenceConfig {
            expand_forward_days: 60,
            expand_backward_days: 1,
        };
        let instances = expand_recurring_event(&event, &config);

        // Should get up to 6 instances
        assert!(instances.len() <= 6);
        assert!(!instances.is_empty());
    }

    #[test]
    fn test_event_with_all_fields() {
        let start = Utc::now().date_naive().and_hms_opt(10, 0, 0).unwrap();
        let start = Utc.from_utc_datetime(&start);
        let end = start + chrono::Duration::hours(1);

        let event = CalendarEvent {
            uid: "full-event".to_string(),
            summary: "Complete Event".to_string(),
            description: Some("Description".to_string()),
            location: Some("Office".to_string()),
            start,
            end,
            calendar_name: "Work".to_string(),
            calendar_url: "/work".to_string(),
            calendar_color: Some("#0000FF".to_string()),
            all_day: false,
            rrule: Some("FREQ=DAILY;COUNT=2".to_string()),
            status: Some("CONFIRMED".to_string()),
            etag: Some("etag123".to_string()),
        };

        let config = RecurrenceConfig::default();
        let instances = expand_recurring_event(&event, &config);

        // Verify all fields are preserved in instances
        assert_eq!(instances.len(), 2);
        for instance in &instances {
            assert_eq!(instance.summary, "Complete Event");
            assert_eq!(instance.description, Some("Description".to_string()));
            assert_eq!(instance.location, Some("Office".to_string()));
            assert_eq!(instance.calendar_color, Some("#0000FF".to_string()));
            assert_eq!(instance.status, Some("CONFIRMED".to_string()));
            assert_eq!(instance.etag, Some("etag123".to_string()));
        }
    }

    #[test]
    fn test_expand_stops_at_window_end() {
        let start = Utc::now().date_naive().and_hms_opt(12, 0, 0).unwrap();
        let start = Utc.from_utc_datetime(&start);
        let end = start + chrono::Duration::hours(1);

        // Many occurrences
        let event = create_test_event(
            "Many occurrences",
            start,
            end,
            Some("FREQ=DAILY;COUNT=1000".to_string()),
        );

        let config = RecurrenceConfig {
            expand_forward_days: 10,
            expand_backward_days: 1,
        };
        let instances = expand_recurring_event(&event, &config);

        // Should stop at window end, not generate all 1000
        assert!(instances.len() < 1000);
        assert!(instances.len() <= 11); // At most 11 days in window
    }

    #[test]
    fn test_normalize_until_already_normalized() {
        // Test that already normalized UNTIL stays the same
        let rrule = "FREQ=WEEKLY;UNTIL=20260315T120000Z;INTERVAL=2";
        let normalized = normalize_until_to_utc(rrule);
        assert_eq!(normalized, rrule);
    }

    #[test]
    fn test_expand_before_window_start() {
        // Event that starts in the past
        let past_date = Utc::now() - chrono::Duration::days(100);
        let start = past_date.date_naive().and_hms_opt(10, 0, 0).unwrap();
        let start = Utc.from_utc_datetime(&start);
        let end = start + chrono::Duration::hours(1);

        let event = create_test_event(
            "Old recurring",
            start,
            end,
            Some("FREQ=DAILY;COUNT=200".to_string()),
        );

        let config = RecurrenceConfig {
            expand_forward_days: 30,
            expand_backward_days: 30,
        };
        let instances = expand_recurring_event(&event, &config);

        // Should skip instances before window start
        // and only include those within the window
        for instance in &instances {
            let now = Utc::now();
            let window_start = now - chrono::Duration::days(30);
            assert!(
                instance.start >= window_start,
                "Instance should be after window start"
            );
        }
    }
}
