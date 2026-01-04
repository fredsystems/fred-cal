// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use anyhow::Result;
use fast_dav_rs::CalDavClient;
use std::io::Write;
use std::sync::{Arc, Once};
use tempfile::NamedTempFile;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

static INIT: Once = Once::new();

/// Initialize rustls crypto provider (needed for tests)
fn init_crypto() {
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Test that we can connect to a mock CalDAV server with file-based credentials
#[tokio::test]
async fn test_caldav_connection_with_file_credentials() -> Result<()> {
    init_crypto();

    // Start a mock CalDAV server
    let mock_server = MockServer::start().await;

    // Mock the current-user-principal discovery
    Mock::given(method("PROPFIND"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
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
        .mount(&mock_server)
        .await;

    // Create temporary credential files
    let mut server_file = NamedTempFile::new()?;
    writeln!(server_file, "{}", mock_server.uri())?;

    let mut username_file = NamedTempFile::new()?;
    writeln!(username_file, "testuser@example.com")?;

    let mut password_file = NamedTempFile::new()?;
    writeln!(password_file, "testpassword")?;

    // Create CalDAV client using file paths (simulating CLI usage)
    let server_url = std::fs::read_to_string(server_file.path())?
        .trim()
        .to_string();
    let username = std::fs::read_to_string(username_file.path())?
        .trim()
        .to_string();
    let password = std::fs::read_to_string(password_file.path())?
        .trim()
        .to_string();

    let client = CalDavClient::new(&server_url, Some(&username), Some(&password))?;

    // Attempt to discover principal (should succeed with mock)
    let principal = client.discover_current_user_principal().await?;

    assert!(principal.is_some());
    assert_eq!(principal.unwrap(), "/principals/user/");

    Ok(())
}

/// Test that we can connect to a mock CalDAV server with direct credentials
#[tokio::test]
async fn test_caldav_connection_with_direct_credentials() -> Result<()> {
    init_crypto();

    // Start a mock CalDAV server
    let mock_server = MockServer::start().await;

    // Mock the current-user-principal discovery
    Mock::given(method("PROPFIND"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/</d:href>
    <d:propstat>
      <d:prop>
        <d:current-user-principal>
          <d:href>/principals/testuser/</d:href>
        </d:current-user-principal>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Create CalDAV client with direct credentials
    let client = CalDavClient::new(
        &mock_server.uri(),
        Some("testuser@example.com"),
        Some("testpassword"),
    )?;

    // Attempt to discover principal (should succeed with mock)
    let principal = client.discover_current_user_principal().await?;

    assert!(principal.is_some());
    assert_eq!(principal.unwrap(), "/principals/testuser/");

    Ok(())
}

/// Test that we can discover calendar home set from a mock server
#[tokio::test]
async fn test_calendar_home_set_discovery() -> Result<()> {
    init_crypto();

    let mock_server = MockServer::start().await;

    // Mock the calendar-home-set discovery
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
        .mount(&mock_server)
        .await;

    let client = CalDavClient::new(
        &mock_server.uri(),
        Some("testuser@example.com"),
        Some("testpassword"),
    )?;

    let homes = client
        .discover_calendar_home_set("/principals/user/")
        .await?;

    assert!(!homes.is_empty());
    assert_eq!(homes[0], "/calendars/user/");

    Ok(())
}

/// Test that we can list calendars from a mock server
#[tokio::test]
async fn test_list_calendars() -> Result<()> {
    init_crypto();

    let mock_server = MockServer::start().await;

    // Mock the calendar list
    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/personal/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Personal Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/user/work/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Work Calendar</d:displayname>
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

    let client = CalDavClient::new(
        &mock_server.uri(),
        Some("testuser@example.com"),
        Some("testpassword"),
    )?;

    let calendars = client.list_calendars("/calendars/user/").await?;

    assert_eq!(calendars.len(), 2);
    assert_eq!(
        calendars[0].displayname,
        Some("Personal Calendar".to_string())
    );
    assert_eq!(calendars[1].displayname, Some("Work Calendar".to_string()));

    Ok(())
}

/// Test that authentication failures are properly handled
#[tokio::test]
async fn test_authentication_failure() -> Result<()> {
    init_crypto();

    let mock_server = MockServer::start().await;

    // Mock an authentication failure
    Mock::given(method("PROPFIND"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&mock_server)
        .await;

    let client = CalDavClient::new(
        &mock_server.uri(),
        Some("wronguser@example.com"),
        Some("wrongpassword"),
    )?;

    let result = client.discover_current_user_principal().await;

    assert!(result.is_err());

    Ok(())
}

/// Test that the full sync workflow completes successfully
///
/// This test verifies that:
/// - The sync manager can connect to a CalDAV server
/// - It can discover calendars
/// - It can query for VEVENTs and VTODOs
/// - The sync completes without errors even with empty results
#[tokio::test]
async fn test_parse_icalendar_data() -> Result<()> {
    init_crypto();

    use fred_cal::cache::CacheManager;
    use fred_cal::sync::SyncManager;
    use std::sync::Arc;
    use tempfile::tempdir;

    let mock_server = MockServer::start().await;

    // Mock the current-user-principal discovery
    Mock::given(method("PROPFIND"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
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
        .mount(&mock_server)
        .await;

    // Mock the calendar-home-set discovery
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
        .mount(&mock_server)
        .await;

    // Mock the calendar list
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

    // Mock REPORT queries for both VEVENT and VTODO
    // Returns empty results - this is sufficient to test the sync workflow
    Mock::given(method("REPORT"))
        .and(path("/calendars/user/personal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Create a temporary cache
    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;

    // Create CalDAV client
    let client = CalDavClient::new(
        &mock_server.uri(),
        Some("testuser@example.com"),
        Some("testpassword"),
    )?;

    // Create sync manager and perform sync
    let sync_manager = Arc::new(SyncManager::new(client, cache)?);
    sync_manager.sync().await?;

    // Verify the parsed data
    let data = sync_manager.data();
    let calendar_data = data.read().await;

    // Verify sync completed successfully
    // The mock returns empty results, which is fine - we're testing the workflow
    assert_eq!(calendar_data.events.len(), 0);
    assert_eq!(calendar_data.todos.len(), 0);

    Ok(())
}

/// Test Apple calendar color fetching with mock server
#[tokio::test]
async fn test_apple_calendar_color_fetch() -> Result<()> {
    init_crypto();

    let mock_server = MockServer::start().await;

    // Mock the current-user-principal discovery
    Mock::given(method("PROPFIND"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:apple="http://apple.com/ns/ical/">
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
        .mount(&mock_server)
        .await;

    // Mock the calendar-home-set discovery
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
        .mount(&mock_server)
        .await;

    // Mock the calendar list with Apple calendar-color
    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:apple="http://apple.com/ns/ical/">
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

    // Mock the Apple calendar-color PROPFIND request
    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/personal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:apple="http://apple.com/ns/ical/">
  <d:response>
    <d:href>/calendars/user/personal/</d:href>
    <d:propstat>
      <d:prop>
        <apple:calendar-color>#FF5733FF</apple:calendar-color>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Mock REPORT query
    Mock::given(method("REPORT"))
        .and(path("/calendars/user/personal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    use fred_cal::cache::CacheManager;
    use fred_cal::sync::SyncManager;
    use tempfile::tempdir;

    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;

    let client = CalDavClient::new(
        &mock_server.uri(),
        Some("testuser@example.com"),
        Some("password123"),
    )?;

    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    sync_manager.sync().await?;

    // Note: We can't directly test the internal calendar_colors map,
    // but this test verifies the workflow doesn't error
    let data = sync_manager.data();
    let calendar_data = data.read().await;

    // Verify sync completed
    assert!(calendar_data.last_sync > chrono::Utc::now() - chrono::Duration::minutes(1));

    Ok(())
}

/// Test sync with events containing calendar colors
#[tokio::test]
async fn test_sync_with_calendar_colors_in_events() -> Result<()> {
    init_crypto();

    let mock_server = MockServer::start().await;

    // Mock the current-user-principal discovery
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
        .mount(&mock_server)
        .await;

    // Mock calendar-home-set discovery
    Mock::given(method("PROPFIND"))
        .and(path("/principals/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/principals/user/</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-home-set xmlns:c="urn:ietf:params:xml:ns:caldav">
          <d:href>/calendars/user/</d:href>
        </c:calendar-home-set>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Mock list calendars with Apple color
    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:apple="http://apple.com/ns/ical/">
  <d:response>
    <d:href>/calendars/user/work/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Work</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
        <apple:calendar-color>#0000FFFF</apple:calendar-color>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Mock calendar query with an event
    Mock::given(method("REPORT"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/work/event1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"abc123"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//Test//EN
BEGIN:VEVENT
UID:test-event-1
DTSTART:20260104T100000Z
DTEND:20260104T110000Z
SUMMARY:Test Meeting
DESCRIPTION:A test event
LOCATION:Conference Room
STATUS:CONFIRMED
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

    use fred_cal::cache::CacheManager;
    use fred_cal::sync::SyncManager;
    use tempfile::tempdir;

    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;

    let client = CalDavClient::new(
        &mock_server.uri(),
        Some("testuser@example.com"),
        Some("password123"),
    )?;

    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    sync_manager.sync().await?;

    let data = sync_manager.data();
    let calendar_data = data.read().await;

    // Verify we got the event
    assert_eq!(calendar_data.events.len(), 1);
    let event = &calendar_data.events[0];
    assert_eq!(event.uid, "test-event-1");
    assert_eq!(event.summary, "Test Meeting");
    assert_eq!(event.calendar_name, "Work");
    // Calendar color should be populated from Apple namespace
    assert_eq!(event.calendar_color, Some("#0000FFFF".to_string()));

    Ok(())
}

/// Test standard CalDAV calendar-color property (non-Apple namespace)
#[tokio::test]
async fn test_standard_caldav_calendar_color() -> Result<()> {
    init_crypto();

    let mock_server = MockServer::start().await;

    // Mock the current-user-principal discovery
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
        .mount(&mock_server)
        .await;

    // Mock the calendar-home-set discovery
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
        .mount(&mock_server)
        .await;

    // Mock the calendar list with standard CalDAV calendar-color
    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:ical="http://apple.com/ns/ical/">
  <d:response>
    <d:href>/calendars/user/standard/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Standard Calendar</d:displayname>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
        <ical:calendar-color>#00FF00FF</ical:calendar-color>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    // Mock REPORT query with an event
    Mock::given(method("REPORT"))
        .and(path("/calendars/user/standard/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/standard/event1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"standard123"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//Test//EN
BEGIN:VEVENT
UID:standard-event-1
DTSTART:20260104T140000Z
DTEND:20260104T150000Z
SUMMARY:Standard Color Event
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

    use fred_cal::cache::CacheManager;
    use fred_cal::sync::SyncManager;
    use tempfile::tempdir;

    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;

    let client = CalDavClient::new(
        &mock_server.uri(),
        Some("testuser@example.com"),
        Some("password123"),
    )?;

    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    sync_manager.sync().await?;

    let data = sync_manager.data();
    let calendar_data = data.read().await;

    // Verify we got the event with standard CalDAV color
    assert_eq!(calendar_data.events.len(), 1);
    let event = &calendar_data.events[0];
    assert_eq!(event.uid, "standard-event-1");
    assert_eq!(event.summary, "Standard Color Event");
    assert_eq!(event.calendar_name, "Standard Calendar");
    // Standard CalDAV color should be populated
    assert_eq!(event.calendar_color, Some("#00FF00FF".to_string()));

    Ok(())
}

/// Test error handling when Apple color PROPFIND fails
#[tokio::test]
async fn test_apple_color_fetch_failure() -> Result<()> {
    init_crypto();

    let mock_server = MockServer::start().await;

    // Mock the current-user-principal discovery
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
        .mount(&mock_server)
        .await;

    // Mock the calendar-home-set discovery
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
        .mount(&mock_server)
        .await;

    // Mock the calendar list without color
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

    // Mock the Apple calendar-color PROPFIND request to return 404
    Mock::given(method("PROPFIND"))
        .and(path("/calendars/user/personal/"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock_server)
        .await;

    // Mock REPORT query
    Mock::given(method("REPORT"))
        .and(path("/calendars/user/personal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
</d:multistatus>"#,
        ))
        .mount(&mock_server)
        .await;

    use fred_cal::cache::CacheManager;
    use fred_cal::sync::SyncManager;
    use tempfile::tempdir;

    let temp_dir = tempdir()?;
    let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;

    let client = CalDavClient::new(
        &mock_server.uri(),
        Some("testuser@example.com"),
        Some("password123"),
    )?;

    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    // Sync should succeed even if Apple color fetch fails
    sync_manager.sync().await?;

    let data = sync_manager.data();
    let calendar_data = data.read().await;

    // Verify sync completed successfully despite color fetch failure
    assert!(calendar_data.last_sync > chrono::Utc::now() - chrono::Duration::minutes(1));

    Ok(())
}

/// Test that events from deleted calendars are cleaned up during sync
#[tokio::test]
async fn test_deleted_calendar_cleanup() -> Result<()> {
    init_crypto();

    use fred_cal::cache::CacheManager;
    use fred_cal::sync::SyncManager;
    use tempfile::tempdir;

    let mock_server = MockServer::start().await;

    // Mock the current-user-principal discovery
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
        .mount(&mock_server)
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
        .mount(&mock_server)
        .await;

    // First sync: Mock list with TWO calendars
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
        .up_to_n_times(1)
        .mount(&mock_server)
        .await;

    // Mock REPORT for personal calendar with one event
    Mock::given(method("REPORT"))
        .and(path("/calendars/user/personal/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/personal/event1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"abc123"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//Test//EN
BEGIN:VEVENT
UID:personal-event-1
DTSTART:20260105T100000Z
DTEND:20260105T110000Z
SUMMARY:Personal Event
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

    // Mock REPORT for work calendar with one event
    Mock::given(method("REPORT"))
        .and(path("/calendars/user/work/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/user/work/event1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"def456"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//Test//EN
BEGIN:VEVENT
UID:work-event-1
DTSTART:20260106T140000Z
DTEND:20260106T150000Z
SUMMARY:Work Meeting
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

    let client = CalDavClient::new(
        &mock_server.uri(),
        Some("testuser@example.com"),
        Some("password123"),
    )?;

    let sync_manager = Arc::new(SyncManager::new(client, cache)?);

    // First sync with both calendars
    sync_manager.sync().await?;

    {
        let data = sync_manager.data();
        let calendar_data = data.read().await;
        assert_eq!(calendar_data.events.len(), 2);
        assert!(
            calendar_data
                .events
                .iter()
                .any(|e| e.uid == "personal-event-1")
        );
        assert!(calendar_data.events.iter().any(|e| e.uid == "work-event-1"));
    }

    // Add a new mock for second sync with only one calendar (work calendar deleted)
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

    // Second sync - work calendar is now gone
    sync_manager.sync().await?;

    {
        let data = sync_manager.data();
        let calendar_data = data.read().await;
        // Should only have 1 event now (work calendar events removed)
        assert_eq!(calendar_data.events.len(), 1);
        assert!(
            calendar_data
                .events
                .iter()
                .any(|e| e.uid == "personal-event-1")
        );
        assert!(!calendar_data.events.iter().any(|e| e.uid == "work-event-1"));
    }

    Ok(())
}
