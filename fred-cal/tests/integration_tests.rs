// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use anyhow::Result;
use fast_dav_rs::CalDavClient;
use std::io::Write;
use tempfile::NamedTempFile;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

/// Test that we can connect to a mock CalDAV server with file-based credentials
#[tokio::test]
async fn test_caldav_connection_with_file_credentials() -> Result<()> {
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
