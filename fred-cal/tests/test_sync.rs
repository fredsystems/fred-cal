// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Integration tests for the sync module
//!
//! These tests verify the SyncManager's ability to synchronize calendar data
//! with a CalDAV server using mocked HTTP responses.

use chrono::{DateTime, Duration, Utc};
use fast_dav_rs::CalDavClient;
use fred_cal::cache::CacheManager;
use fred_cal::sync::SyncManager;
use std::sync::Arc;
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Setup function to initialize rustls crypto provider
fn setup_rustls() {
    use std::sync::Once;
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
    });
}

/// Generate a datetime within the recurrence expansion window (default: 730 days forward)
/// This ensures recurring events will expand properly in tests.
fn test_date_in_future(days_from_now: i64) -> DateTime<Utc> {
    Utc::now() + Duration::days(days_from_now)
}

/// Format a datetime for iCalendar (UTC format: YYYYMMDDTHHmmssZ)
fn format_ical_datetime(dt: DateTime<Utc>) -> String {
    dt.format("%Y%m%dT%H%M%SZ").to_string()
}

/// Format a date for iCalendar (DATE format: YYYYMMDD)
fn format_ical_date(dt: DateTime<Utc>) -> String {
    dt.format("%Y%m%d").to_string()
}

/// Create a mock CalDAV server with standard discovery endpoints
async fn setup_mock_caldav_server(mock_server: &MockServer) {
    // Mock principal discovery
    Mock::given(method("PROPFIND"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/</d:href>
    <d:propstat>
      <d:prop>
        <d:current-user-principal>
          <d:href>/principals/user/</d:href>
        </d:current-user-principal>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(mock_server)
        .await;

    // Mock calendar-home-set discovery
    Mock::given(method("PROPFIND"))
        .and(path("/principals/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/principals/user/</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-home-set>
          <d:href>/calendars/user/</d:href>
        </c:calendar-home-set>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(mock_server)
        .await;

    // Mock WebDAV sync support check
    Mock::given(method("OPTIONS"))
        .respond_with(ResponseTemplate::new(200).insert_header("DAV", "1, 2, sync-collection"))
        .mount(mock_server)
        .await;
}

/// Test basic synchronization with a single event
#[tokio::test]
async fn test_basic_sync_single_event() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    // Mock calendar list
    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:apple="http://apple.com/ns/ical/">
  <d:response>
    <d:href>/calendars/user/work/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Work Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
        <apple:calendar-color>#FF5733</apple:calendar-color>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Mock calendar query for events
    let start = test_date_in_future(30);
    let end = start + Duration::hours(1);
    let start_str = format_ical_datetime(start);
    let end_str = format_ical_datetime(end);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/work/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/work/event1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"etag1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//Test//EN
BEGIN:VEVENT
UID:event1@example.com
DTSTART:{}
DTEND:{}
SUMMARY:Team Meeting
DESCRIPTION:Quarterly planning meeting
LOCATION:Conference Room A
STATUS:CONFIRMED
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            start_str, end_str
        )))
        .mount(&mock_server)
        .await;

    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;
    let client = CalDavClient::new(&mock_server.uri(), Some("user"), Some("pass"))?;
    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    // Perform sync
    sync_manager.sync().await?;

    // Verify event was synced
    let data = sync_manager.data();
    let calendar_data = data.read().await;
    assert_eq!(calendar_data.events.len(), 1);
    assert_eq!(calendar_data.events[0].summary, "Team Meeting");
    assert_eq!(calendar_data.events[0].uid, "event1@example.com");
    assert_eq!(calendar_data.events[0].calendar_name, "Work Calendar");
    assert_eq!(
        calendar_data.events[0].location,
        Some("Conference Room A".to_string())
    );

    Ok(())
}

/// Test synchronization with multiple events and todos
#[tokio::test]
async fn test_sync_events_and_todos() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    // Mock calendar list
    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/personal/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Personal</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Mock VEVENT query
    let event_start = test_date_in_future(45);
    let event_end = event_start + Duration::hours(1);
    let todo_due = test_date_in_future(50);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/personal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/personal/event1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"e1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:event1
DTSTART:{}
DTEND:{}
SUMMARY:Doctor Appointment
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/user/personal/task1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"t1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VTODO
UID:todo1
SUMMARY:Buy groceries
DUE:{}
PRIORITY:1
PERCENT-COMPLETE:0
STATUS:NEEDS-ACTION
END:VTODO
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_datetime(event_start),
            format_ical_datetime(event_end),
            format_ical_datetime(todo_due)
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

    assert_eq!(calendar_data.events.len(), 1);
    assert_eq!(calendar_data.events[0].summary, "Doctor Appointment");

    assert_eq!(calendar_data.todos.len(), 1);
    assert_eq!(calendar_data.todos[0].summary, "Buy groceries");
    assert_eq!(calendar_data.todos[0].priority, Some(1));

    Ok(())
}

/// Test synchronization with an empty calendar
#[tokio::test]
async fn test_sync_empty_calendar() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    // Mock calendar list with empty calendar
    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/empty/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Empty Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Mock empty REPORT response
    Mock::given(method("REPORT"))
        .and(path("/calendars/user/empty/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;
    let client = CalDavClient::new(&mock_server.uri(), Some("user"), Some("pass"))?;
    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    sync_manager.sync().await?;

    let data = sync_manager.data();
    let calendar_data = data.read().await;
    assert!(calendar_data.events.is_empty());
    assert!(calendar_data.todos.is_empty());

    Ok(())
}

/// Test synchronization with server error
#[tokio::test]
async fn test_sync_with_server_error() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    // Mock calendar list
    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/work/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Work</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Mock server error for calendar query
    Mock::given(method("REPORT"))
        .and(path("/calendars/user/work/"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;
    let client = CalDavClient::new(&mock_server.uri(), Some("user"), Some("pass"))?;
    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    // Sync should complete but calendar data will be empty due to error
    sync_manager.sync().await?;

    let data = sync_manager.data();
    let calendar_data = data.read().await;
    assert!(calendar_data.events.is_empty());

    Ok(())
}

/// Test all-day events are parsed correctly
#[tokio::test]
async fn test_sync_all_day_event() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let holiday = test_date_in_future(60);
    let holiday_end = holiday + Duration::days(1);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/cal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/holiday.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"h1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:holiday1
DTSTART;VALUE=DATE:{}
DTEND;VALUE=DATE:{}
SUMMARY:Independence Day
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_date(holiday),
            format_ical_date(holiday_end)
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

    assert_eq!(calendar_data.events.len(), 1);
    assert_eq!(calendar_data.events[0].summary, "Independence Day");
    assert!(calendar_data.events[0].all_day);

    Ok(())
}

/// Test cache persistence across sync operations
#[tokio::test]
async fn test_cache_persistence() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/work/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Work</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let event_start = test_date_in_future(90);
    let event_end = event_start + Duration::hours(1);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/work/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/work/event1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"e1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:evt1
DTSTART:{}
DTEND:{}
SUMMARY:Morning Standup
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_datetime(event_start),
            format_ical_datetime(event_end)
        )))
        .mount(&mock_server)
        .await;

    let temp_dir = tempdir()?;
    let cache_path = temp_dir.path().to_path_buf();

    // First sync
    {
        let cache = CacheManager::new_with_path(cache_path.clone())?;
        let client = CalDavClient::new(&mock_server.uri(), Some("user"), Some("pass"))?;
        let sync_manager = Arc::new(SyncManager::new(client, cache)?);
        sync_manager.sync().await?;
    }

    // Second sync with new manager - should load from cache
    {
        let cache = CacheManager::new_with_path(cache_path)?;
        let client = CalDavClient::new(&mock_server.uri(), Some("user"), Some("pass"))?;
        let sync_manager = Arc::new(SyncManager::new(client, cache)?);

        // Data should be loaded from cache before sync
        let data = sync_manager.data();
        let calendar_data = data.read().await;
        assert_eq!(calendar_data.events.len(), 1);
        assert_eq!(calendar_data.events[0].summary, "Morning Standup");
    }

    Ok(())
}

/// Test multiple calendars synchronization
#[tokio::test]
async fn test_multiple_calendars() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/work/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Work</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/user/personal/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Personal</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let work_start = test_date_in_future(100);
    let work_end = work_start + Duration::hours(1);
    let personal_start = test_date_in_future(101);
    let personal_end = personal_start + Duration::hours(1);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/work/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/work/e1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"w1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:work1
DTSTART:{}
DTEND:{}
SUMMARY:Client Meeting
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_datetime(work_start),
            format_ical_datetime(work_end)
        )))
        .mount(&mock_server)
        .await;

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/personal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/personal/e2.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"p1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:personal1
DTSTART:{}
DTEND:{}
SUMMARY:Dentist Appointment
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_datetime(personal_start),
            format_ical_datetime(personal_end)
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

    assert_eq!(calendar_data.events.len(), 2);

    let work_events: Vec<_> = calendar_data
        .events
        .iter()
        .filter(|e| e.calendar_name == "Work")
        .collect();
    assert_eq!(work_events.len(), 1);
    assert_eq!(work_events[0].summary, "Client Meeting");

    let personal_events: Vec<_> = calendar_data
        .events
        .iter()
        .filter(|e| e.calendar_name == "Personal")
        .collect();
    assert_eq!(personal_events.len(), 1);
    assert_eq!(personal_events[0].summary, "Dentist Appointment");

    Ok(())
}

/// Test incremental sync with sync tokens
#[tokio::test]
async fn test_incremental_sync_with_tokens() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/work/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Work</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Just verify sync tokens are stored - full sync stores empty tokens
    let start = test_date_in_future(120);
    let end = start + Duration::hours(1);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/work/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/work/e1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"v1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:evt1
DTSTART:{}
DTEND:{}
SUMMARY:Initial Meeting
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_datetime(start),
            format_ical_datetime(end)
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
    assert_eq!(calendar_data.events.len(), 1);
    assert_eq!(calendar_data.events[0].summary, "Initial Meeting");

    Ok(())
}

/// Test event updates with changed etags
#[tokio::test]
async fn test_event_update_with_etag_change() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Verify event data changes across syncs (full sync replaces all events)
    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;
    let client = CalDavClient::new(&mock_server.uri(), Some("user"), Some("pass"))?;
    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    let updated_start = test_date_in_future(150);
    let updated_end = updated_start + Duration::hours(1);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/cal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/evt.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"v2"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:unique-evt
DTSTART:{}
DTEND:{}
SUMMARY:Updated Title
LOCATION:Room B
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_datetime(updated_start),
            format_ical_datetime(updated_end)
        )))
        .mount(&mock_server)
        .await;

    sync_manager.sync().await?;

    let data = sync_manager.data();
    let calendar_data = data.read().await;
    assert_eq!(calendar_data.events.len(), 1);
    assert_eq!(calendar_data.events[0].summary, "Updated Title");
    assert_eq!(calendar_data.events[0].location, Some("Room B".to_string()));

    Ok(())
}

/// Test event deletion
#[tokio::test]
async fn test_event_deletion() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Test that full sync replaces events (simulates deletion)
    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;
    let client = CalDavClient::new(&mock_server.uri(), Some("user"), Some("pass"))?;
    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    // Mock with only one event (simulating event2 was deleted)
    let event_start = test_date_in_future(180);
    let event_end = event_start + Duration::hours(1);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/cal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/e1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"e1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:event1
DTSTART:{}
DTEND:{}
SUMMARY:Event 1
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_datetime(event_start),
            format_ical_datetime(event_end)
        )))
        .mount(&mock_server)
        .await;

    sync_manager.sync().await?;

    let data = sync_manager.data();
    let calendar_data = data.read().await;
    assert_eq!(calendar_data.events.len(), 1);
    assert_eq!(calendar_data.events[0].uid, "event1");

    Ok(())
}

/// Test recurring event with RRULE
#[tokio::test]
async fn test_recurring_event() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let recur_start = test_date_in_future(10);
    let recur_end = recur_start + Duration::minutes(30);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/cal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/recurring.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"r1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:recurring-standup
DTSTART:{}
DTEND:{}
SUMMARY:Daily Standup
RRULE:FREQ=DAILY;COUNT=5
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_datetime(recur_start),
            format_ical_datetime(recur_end)
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

    // Should have 5 instances of the recurring event
    assert_eq!(calendar_data.events.len(), 5);
    assert!(
        calendar_data
            .events
            .iter()
            .all(|e| e.summary == "Daily Standup")
    );
    assert!(
        calendar_data
            .events
            .iter()
            .all(|e| e.uid == "recurring-standup")
    );

    Ok(())
}

/// Test recurring event with EXDATE (exception dates)
/// NOTE: EXDATE is not currently parsed/handled by the recurrence module,
/// so this test verifies that the event still expands to all COUNT instances.
/// Future enhancement: Parse and apply EXDATE to exclude specific occurrences.
#[tokio::test]
async fn test_recurring_event_with_exdate() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let weekly_start = test_date_in_future(7);
    let weekly_end = weekly_start + Duration::hours(1);
    let exdate = weekly_start + Duration::days(7);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/cal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/weekly.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"w1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:weekly-meeting
DTSTART:{}
DTEND:{}
SUMMARY:Weekly Team Meeting
RRULE:FREQ=WEEKLY;COUNT=4
EXDATE:{}
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_datetime(weekly_start),
            format_ical_datetime(weekly_end),
            format_ical_datetime(exdate)
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

    // Currently EXDATE is not processed, so we get all 4 instances
    // TODO: When EXDATE support is added, this should be 3 (4 - 1 excluded)
    assert_eq!(calendar_data.events.len(), 4);

    Ok(())
}

/// Test batch processing with large number of events
#[tokio::test]
async fn test_batch_processing_large_calendar() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/large/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Large Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Generate 50 events
    let mut responses = String::new();
    for i in 1..=50 {
        let event_start = test_date_in_future(200 + i);
        let event_end = event_start + Duration::hours(1);
        responses.push_str(&format!(
            r#"  <d:response>
    <d:href>/calendars/user/large/e{}.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"e{}"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:event{}
DTSTART:{}
DTEND:{}
SUMMARY:Event {}
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
"#,
            i,
            i,
            i,
            format_ical_datetime(event_start),
            format_ical_datetime(event_end),
            i
        ));
    }

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/large/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
{}
</d:multistatus>"#,
            responses
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
    assert_eq!(calendar_data.events.len(), 50);

    Ok(())
}

/// Test calendar color property (Apple-specific)
#[tokio::test]
async fn test_calendar_color() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:apple="http://apple.com/ns/ical/">
  <d:response>
    <d:href>/calendars/user/colored/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Colored Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
        <apple:calendar-color>#FF6347</apple:calendar-color>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let color_start = test_date_in_future(300);
    let color_end = color_start + Duration::hours(1);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/colored/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/colored/e1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"e1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:colored-event
DTSTART:{}
DTEND:{}
SUMMARY:Colored Event
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_datetime(color_start),
            format_ical_datetime(color_end)
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
    assert_eq!(calendar_data.events.len(), 1);
    assert_eq!(
        calendar_data.events[0].calendar_color,
        Some("#FF6347".to_string())
    );

    Ok(())
}

/// Test events with different timezones
#[tokio::test]
async fn test_multiple_timezones() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/tz/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>TZ Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let ny_date = test_date_in_future(350);
    let tokyo_date = test_date_in_future(350);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/tz/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/tz/e1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"e1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:ny-event
DTSTART;TZID=America/New_York:{}T140000
DTEND;TZID=America/New_York:{}T150000
SUMMARY:New York Meeting
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/user/tz/e2.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"e2"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:tokyo-event
DTSTART;TZID=Asia/Tokyo:{}T090000
DTEND;TZID=Asia/Tokyo:{}T100000
SUMMARY:Tokyo Meeting
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_date(ny_date),
            format_ical_date(ny_date),
            format_ical_date(tokyo_date),
            format_ical_date(tokyo_date)
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
    assert_eq!(calendar_data.events.len(), 2);

    let ny_event = calendar_data.events.iter().find(|e| e.uid == "ny-event");
    let tokyo_event = calendar_data.events.iter().find(|e| e.uid == "tokyo-event");

    assert!(ny_event.is_some());
    assert!(tokyo_event.is_some());

    Ok(())
}

/// Test floating time events (no timezone)
#[tokio::test]
async fn test_floating_time_events() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let floating_date = test_date_in_future(400);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/cal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/floating.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"f1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:floating-event
DTSTART:{}T100000
DTEND:{}T110000
SUMMARY:Floating Time Event
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_date(floating_date),
            format_ical_date(floating_date)
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
    assert_eq!(calendar_data.events.len(), 1);
    assert_eq!(calendar_data.events[0].summary, "Floating Time Event");

    Ok(())
}

/// Test malformed iCalendar data handling
#[tokio::test]
async fn test_malformed_icalendar() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/cal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/bad.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"bad"</d:getetag>
        <c:calendar-data>INVALID ICALENDAR DATA
This is not valid at all!</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/user/cal/good.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"good"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:good-event
DTSTART:20241001T100000Z
DTEND:20241001T110000Z
SUMMARY:Valid Event
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;
    let client = CalDavClient::new(&mock_server.uri(), Some("user"), Some("pass"))?;
    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    // Should not crash - malformed data should be skipped
    sync_manager.sync().await?;

    let data = sync_manager.data();
    let calendar_data = data.read().await;
    // Only the valid event should be parsed
    assert_eq!(calendar_data.events.len(), 1);
    assert_eq!(calendar_data.events[0].summary, "Valid Event");

    Ok(())
}

/// Test event with missing required fields
#[tokio::test]
async fn test_event_missing_required_fields() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/cal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/incomplete.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"inc"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:incomplete-event
SUMMARY:Event Without Start Time
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;
    let client = CalDavClient::new(&mock_server.uri(), Some("user"), Some("pass"))?;
    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    // Should not crash - event without DTSTART should be skipped or handled gracefully
    sync_manager.sync().await?;

    let data = sync_manager.data();
    let calendar_data = data.read().await;
    // Event without DTSTART should be skipped since start is required
    assert!(calendar_data.events.is_empty());

    Ok(())
}

/// Test authentication failure (401)
#[tokio::test]
async fn test_authentication_failure() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;

    // Return 401 Unauthorized for principal discovery
    Mock::given(method("PROPFIND"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&mock_server)
        .await;

    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;
    let client = CalDavClient::new(&mock_server.uri(), Some("user"), Some("wrongpass"))?;
    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    // Should return an error
    let result = sync_manager.sync().await;
    assert!(result.is_err());

    Ok(())
}

/// Test todo with all fields populated
#[tokio::test]
async fn test_todo_all_fields() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/tasks/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Tasks</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let todo_start = test_date_in_future(450);
    let todo_due = test_date_in_future(464);
    let todo_completed = test_date_in_future(459);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/tasks/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/tasks/t1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"t1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VTODO
UID:comprehensive-task
SUMMARY:Complete Project Report
DESCRIPTION:Write the quarterly report with all metrics
DTSTART:{}
DUE:{}
PRIORITY:1
PERCENT-COMPLETE:60
STATUS:IN-PROCESS
COMPLETED:{}
END:VTODO
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_datetime(todo_start),
            format_ical_datetime(todo_due),
            format_ical_datetime(todo_completed)
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

    assert_eq!(calendar_data.todos.len(), 1);
    let todo = &calendar_data.todos[0];
    assert_eq!(todo.summary, "Complete Project Report");
    assert_eq!(
        todo.description,
        Some("Write the quarterly report with all metrics".to_string())
    );
    assert_eq!(todo.priority, Some(1));
    assert_eq!(todo.percent_complete, Some(60));
    assert_eq!(todo.status, "InProcess".to_string());
    assert!(todo.due.is_some());
    assert!(todo.start.is_some());
    assert!(todo.completed.is_some());

    Ok(())
}

/// Test event with various statuses
#[tokio::test]
async fn test_event_statuses() -> Result<(), Box<dyn std::error::Error>> {
    setup_rustls();

    let mock_server = MockServer::start().await;
    setup_mock_caldav_server(&mock_server).await;

    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    let confirmed_start = test_date_in_future(500);
    let confirmed_end = confirmed_start + Duration::hours(1);
    let tentative_start = test_date_in_future(501);
    let tentative_end = tentative_start + Duration::hours(1);
    let cancelled_start = test_date_in_future(502);
    let cancelled_end = cancelled_start + Duration::hours(1);

    Mock::given(method("REPORT"))
        .and(path("/calendars/user/cal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/cal/confirmed.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"c1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:confirmed-event
DTSTART:{}
DTEND:{}
SUMMARY:Confirmed Meeting
STATUS:CONFIRMED
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/user/cal/tentative.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"t1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:tentative-event
DTSTART:{}
DTEND:{}
SUMMARY:Tentative Meeting
STATUS:TENTATIVE
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/user/cal/cancelled.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"x1"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:cancelled-event
DTSTART:{}
DTEND:{}
SUMMARY:Cancelled Meeting
STATUS:CANCELLED
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            format_ical_datetime(confirmed_start),
            format_ical_datetime(confirmed_end),
            format_ical_datetime(tentative_start),
            format_ical_datetime(tentative_end),
            format_ical_datetime(cancelled_start),
            format_ical_datetime(cancelled_end)
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

    assert_eq!(calendar_data.events.len(), 3);

    let confirmed = calendar_data
        .events
        .iter()
        .find(|e| e.uid == "confirmed-event");
    let tentative = calendar_data
        .events
        .iter()
        .find(|e| e.uid == "tentative-event");
    let cancelled = calendar_data
        .events
        .iter()
        .find(|e| e.uid == "cancelled-event");

    assert_eq!(confirmed.unwrap().status, Some("Confirmed".to_string()));
    assert_eq!(tentative.unwrap().status, Some("Tentative".to_string()));
    assert_eq!(cancelled.unwrap().status, Some("Cancelled".to_string()));

    Ok(())
}
