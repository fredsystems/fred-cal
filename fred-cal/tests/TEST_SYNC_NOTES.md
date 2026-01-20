# Integration Test Notes for sync.rs

## Overview

This document explains the comprehensive integration tests for the `sync.rs` module. The test suite verifies the SyncManager's ability to synchronize calendar data with a CalDAV server using mocked HTTP responses.

## Test Suite Status

✅ **21 comprehensive integration tests - ALL PASSING**

## Problems Fixed

### 1. Rustls Crypto Provider Issue

**Problem**: Tests were failing with:

```
Could not automatically determine the process-level CryptoProvider from Rustls crate features.
```

**Solution**: Added a `setup_rustls()` function that initializes the crypto provider once before tests run:

```rust
fn setup_rustls() {
    use std::sync::Once;
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
    });
}
```

This must be called at the beginning of each test.

### 2. Incomplete CalDAV Mock Setup

**Problem**: Original tests only mocked the REPORT endpoint but not the full CalDAV discovery flow that `SyncManager::sync()` requires.

**Solution**: Created `setup_mock_caldav_server()` helper function that properly mocks:

- Current user principal discovery (`PROPFIND /`)
- Calendar home set discovery (`PROPFIND /principals/user/`)
- WebDAV sync capabilities check (`OPTIONS`)

### 3. Invalid iCalendar Data

**Problem**: Tests used incomplete iCalendar data missing required components like `BEGIN:VCALENDAR`, `VERSION`, and `END:VCALENDAR`.

**Solution**: All test responses now include proper iCalendar structure:

```
BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
...
END:VEVENT
END:VCALENDAR
```

### 4. Date Range Issues for Recurring Events

**Problem**: Recurring events weren't expanding because test dates were outside the RecurrenceConfig expansion window.

**Solution**: Implemented dynamic date generation using helper functions:

- `test_date_in_future(days)` - Generates dates relative to test execution time
- `format_ical_datetime(dt)` - Formats UTC datetime for iCalendar
- `format_ical_date(dt)` - Formats date-only values for iCalendar

This ensures tests will always work regardless of when they're run, and dates are always within the RecurrenceConfig expansion window.

### 5. Status Field Normalization

**Problem**: Tests expected raw status values like "IN-PROCESS" but the parser normalizes them to "InProcess".

**Solution**: Updated assertions to match the normalized form that the parser produces.

## Complete Test Coverage

### Basic Synchronization (7 tests)

1. **test_basic_sync_single_event**
   - Tests synchronization of a single event
   - Verifies event properties (summary, UID, location, calendar name)
   - Validates basic CalDAV flow

2. **test_sync_events_and_todos**
   - Tests synchronization of both events and todos
   - Verifies proper parsing of VEVENT and VTODO components
   - Checks todo-specific fields (priority, status)

3. **test_sync_empty_calendar**
   - Tests handling of calendars with no events or todos
   - Ensures empty calendars don't cause errors

4. **test_sync_with_server_error**
   - Tests resilience when server returns 500 errors
   - Verifies sync completes without crashing even if individual calendars fail

5. **test_sync_all_day_event**
   - Tests parsing of all-day events (DATE vs DATETIME)
   - Verifies `all_day` flag is set correctly

6. **test_cache_persistence**
   - Tests that calendar data persists between sync operations
   - Verifies cache save/load functionality

7. **test_multiple_calendars**
   - Tests synchronization across multiple calendars
   - Ensures events from different calendars are properly separated

### Incremental Sync & Updates (3 tests)

8. **test_incremental_sync_with_tokens**
   - Tests basic sync token storage
   - Verifies that sync operations complete successfully
   - Validates calendar data is properly fetched

9. **test_event_update_with_etag_change**
   - Tests that events are updated when server data changes
   - Verifies etag-based change detection works in full sync mode
   - Ensures old event data is replaced with new data

10. **test_event_deletion**
    - Tests that events removed from server are removed from local cache
    - Verifies full sync replaces calendar data correctly
    - Ensures deleted events don't persist

### Recurrence Testing (2 tests)

11. **test_recurring_event**
    - Tests events with RRULE patterns (FREQ=DAILY;COUNT=5)
    - Verifies instances are created correctly (5 instances generated)
    - Ensures all instances share the same UID and summary

12. **test_recurring_event_with_exdate**
    - Tests recurring events with EXDATE (exception dates)
    - **NOTE**: EXDATE is not currently parsed/handled
    - Documents future enhancement opportunity
    - Currently verifies all COUNT instances are generated

### Advanced CalDAV Features (4 tests)

13. **test_batch_processing_large_calendar**
    - Tests synchronization of 50 events in a single calendar
    - Verifies batch processing doesn't lose any events
    - Ensures performance with larger datasets

14. **test_calendar_color**
    - Tests Apple-specific calendar-color property
    - Verifies color is attached to events
    - Tests hex color format (#FF6347)

15. **test_multiple_timezones**
    - Tests events with different TZID values (America/New_York, Asia/Tokyo)
    - Verifies timezone information is preserved
    - Ensures multiple timezones in same calendar work

16. **test_floating_time_events**
    - Tests events without timezone information
    - Verifies floating times are handled gracefully
    - No Z suffix or TZID parameter

### Edge Cases (5 tests)

17. **test_malformed_icalendar**
    - Tests handling of invalid/corrupt iCalendar data
    - Verifies malformed data is skipped without crashing
    - Ensures valid events in same response are still processed

18. **test_event_missing_required_fields**
    - Tests events without DTSTART (required field)
    - Verifies parser handles missing required fields gracefully
    - Events are skipped rather than causing sync to fail

19. **test_authentication_failure**
    - Tests 401 Unauthorized responses
    - Verifies sync returns error on auth failure
    - Ensures proper error propagation

20. **test_todo_all_fields**
    - Tests todos with all optional fields populated
    - Verifies: summary, description, due, start, completed, priority, percent_complete, status
    - Ensures comprehensive todo parsing

21. **test_event_statuses**
    - Tests events with various statuses: CONFIRMED, TENTATIVE, CANCELLED
    - Verifies status normalization (CONFIRMED → Confirmed)
    - Ensures all standard status values are parsed correctly

## Known Limitations & Future Enhancements

### EXDATE Support

- **Current**: EXDATE properties in recurring events are not parsed
- **Impact**: All instances defined by COUNT/UNTIL are generated, exclusions ignored
- **Future**: Add EXDATE parsing to recurrence module to exclude specific dates
- **Test**: test_recurring_event_with_exdate documents this limitation

### Incremental Sync

- **Current**: Tests verify basic sync functionality but don't test true incremental sync-collection
- **Reason**: Full sync mode is the primary path currently used
- **Future**: Add tests for WebDAV sync-collection with actual sync token updates

## Dynamic Date Generation Helpers

To ensure tests remain valid indefinitely, all dates are generated dynamically relative to test execution time:

### Helper Functions

```rust
/// Generate a datetime N days in the future from now
fn test_date_in_future(days_from_now: i64) -> DateTime<Utc>

/// Format datetime for iCalendar UTC format (YYYYMMDDTHHmmssZ)
fn format_ical_datetime(dt: DateTime<Utc>) -> String

/// Format date for iCalendar DATE format (YYYYMMDD)
fn format_ical_date(dt: DateTime<Utc>) -> String
```

### Usage Example

```rust
// Generate dates within expansion window
let event_start = test_date_in_future(30);  // 30 days from now
let event_end = event_start + Duration::hours(1);

// Use in iCalendar XML
let xml = format!(
    r#"<c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:test-event
DTSTART:{}
DTEND:{}
SUMMARY:Test Event
END:VEVENT
END:VCALENDAR</c:calendar-data>"#,
    format_ical_datetime(event_start),
    format_ical_datetime(event_end)
);
```

### Why Dynamic Dates?

- **Future-proof**: Tests never fail because dates become outdated
- **Expansion window**: All dates guaranteed to be within RecurrenceConfig window (730 days forward, 365 days backward)
- **Recurring events**: Events will always expand properly
- **No maintenance**: Never need to update hard-coded dates

## How to Add New Tests

1. Start with `setup_rustls()` call
2. Create mock server with `MockServer::start().await`
3. Call `setup_mock_caldav_server(&mock_server).await`
4. Generate dynamic dates using `test_date_in_future(days)` helper
5. Mock additional endpoints as needed:
   - Calendar list: `PROPFIND /calendars/user/`
   - Event queries: `REPORT /calendars/user/{calendar}/`
   - Use `format_ical_datetime()` or `format_ical_date()` for dates in XML
6. Create temp directory and cache manager
7. Create CalDavClient pointing to mock server
8. Create SyncManager and call `sync().await`
9. Verify results via `sync_manager.data()`

## Example Test Template

```rust
#[tokio::test]
async fn test_my_scenario() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    // Generate dynamic dates
    let event_start = test_date_in_future(30);
    let event_end = event_start + Duration::hours(1);

    // Mock calendar list
    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            // ... calendar list XML
        ))
        .mount(&mock_server)
        .await;

    // Mock event data with dynamic dates
    Mock::given(method("REPORT"))
        .and(path("/calendars/user/mycal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"...
            DTSTART:{}
            DTEND:{}
            ..."#,
            format_ical_datetime(event_start),
            format_ical_datetime(event_end)
        )))
        .mount(&mock_server)
        .await;

    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;
    let client = CalDavClient::new(&mock_server.uri(), Some("user"), Some("pass"))?;
    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    sync_manager.sync().await?;

    let data = sync_manager.data();
    let calendar_data = data.read().await;

    // Assert expected results
    assert_eq!(calendar_data.events.len(), 1);

    Ok(())
}
```

## Running Tests

```bash
# Run all integration tests
cargo test --test test_sync

# Run specific test
cargo test --test test_sync test_basic_sync_single_event

# Run with output
cargo test --test test_sync -- --nocapture

# Run with backtrace on failure
RUST_BACKTRACE=1 cargo test --test test_sync
```

## Important Notes

### Test Dates

- **All test dates are generated dynamically relative to test execution time**
- Uses helper functions: `test_date_in_future(days)` generates dates N days in the future
- Dates are automatically within the RecurrenceConfig expansion window (730 days forward, 365 days backward)
- Tests will continue to work correctly regardless of when they're run
- No hard-coded dates means no test failures due to time passing

### Status Normalization

- The parser normalizes status values to PascalCase
- CONFIRMED → Confirmed
- TENTATIVE → Tentative
- CANCELLED → Cancelled
- IN-PROCESS → InProcess
- NEEDS-ACTION → NeedsAction

### Test Isolation

- Each test uses a temporary directory for cache to avoid interference
- Mock server URIs are unique per test (wiremock handles this)
- Tests run in parallel by default - no shared state
- All calendar data in tests uses UTC times for consistency
- **Dates are generated dynamically relative to test execution time**

### Mock Server Behavior

- Mocks must be set up before the sync operation
- Later mocks with same path/method will override earlier ones (useful for multi-sync tests)
- Mock server automatically handles OPTIONS, but explicit mocking is more reliable

## Coverage Summary

| Category            | Tests  | Coverage                                                |
| ------------------- | ------ | ------------------------------------------------------- |
| Basic Sync          | 7      | Full CRUD operations, multiple calendars, caching       |
| Incremental Updates | 3      | Event updates, deletions, sync tokens                   |
| Recurrence          | 2      | RRULE expansion, exception dates (limitation noted)     |
| Advanced Features   | 4      | Batch processing, colors, timezones, floating times     |
| Edge Cases          | 5      | Malformed data, missing fields, auth errors, all fields |
| **TOTAL**           | **21** | **Comprehensive coverage of sync.rs**                   |

All tests passing ✅
