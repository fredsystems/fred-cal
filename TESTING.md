# Testing Documentation

## Overview

This project uses a comprehensive testing strategy that covers both unit tests and integration tests. All tests are designed to run without hitting real external services, ensuring fast, reliable, and isolated test execution.

## Test Structure

### Unit Tests (`fred-cal/src/cli.rs`)

Located directly in the source files using Rust's `#[cfg(test)]` modules.

**Coverage:**

- Direct value loading
- File-based value loading
- Whitespace trimming from files
- URL validation (http/https scheme enforcement)
- Empty credential validation
- Invalid URL scheme rejection

**Run unit tests only:**

```bash
cargo test --lib
```

### Integration Tests (`fred-cal/tests/integration_tests.rs`)

Full end-to-end tests using a mock CalDAV server powered by `wiremock`.

**Coverage:**

- CalDAV connection with file-based credentials
- CalDAV connection with direct credentials
- Current user principal discovery
- Calendar home set discovery
- Calendar listing
- Authentication failure handling

**Run integration tests only:**

```bash
cargo test --test integration_tests
```

## Mock CalDAV Server

We use the `wiremock` crate to create a local HTTP server that simulates CalDAV responses. This provides several benefits:

1. **No external dependencies**: Tests don't require internet access or real CalDAV servers
2. **Fast execution**: No network latency
3. **Predictable results**: Controlled responses ensure consistent test outcomes
4. **No rate limiting**: Can run tests repeatedly without hitting API limits
5. **Privacy**: No credentials or test data sent to external services

### Example Mock Setup

```rust
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path}};

let mock_server = MockServer::start().await;

Mock::given(method("PROPFIND"))
    .and(path("/"))
    .respond_with(ResponseTemplate::new(207).set_body_string(
        r#"<?xml version="1.0" encoding="UTF-8"?>
        <d:multistatus xmlns:d="DAV:">
          <!-- CalDAV XML response -->
        </d:multistatus>"#,
    ))
    .mount(&mock_server)
    .await;

let client = CalDavClient::new(
    &mock_server.uri(),  // Use mock server URL
    Some("testuser"),
    Some("testpass"),
)?;
```

## Running Tests

### Run all tests

```bash
cargo test
```

### Run with output visible

```bash
cargo test -- --nocapture
```

### Run a specific test

```bash
cargo test test_caldav_connection_with_file_credentials
```

### Run tests with verbose output

```bash
cargo test -- --test-threads=1 --nocapture
```

## Code Coverage

All critical code paths are tested:

- ✅ CLI argument parsing
- ✅ File vs. direct value detection
- ✅ File reading and trimming
- ✅ URL validation
- ✅ Credential validation
- ✅ CalDAV client initialization
- ✅ CalDAV principal discovery
- ✅ Calendar home set discovery
- ✅ Calendar listing
- ✅ Authentication error handling

## Linting and Quality

All tests must pass clippy with strict lints:

```bash
cargo clippy --all-targets -- -D warnings
```

### Enforced Lints

- No `unwrap()` or `expect()` calls
- Proper error handling with `?` operator
- All `Result` types handled appropriately
- Pedantic clippy checks enabled

## CI/CD Considerations

Tests are designed to run in CI environments:

- No external network dependencies
- Deterministic outcomes
- Fast execution (typically < 1 second for all tests)
- No file system pollution (uses `tempfile` crate)
- Clean shutdown (no resource leaks)

## Adding New Tests

### Unit Test Template

```rust
#[test]
fn test_new_feature() -> Result<()> {
    // Arrange
    let input = "test_value";

    // Act
    let result = function_under_test(input)?;

    // Assert
    assert_eq!(result, expected_value);
    Ok(())
}
```

### Integration Test Template

```rust
#[tokio::test]
async fn test_new_caldav_feature() -> Result<()> {
    // Setup mock server
    let mock_server = MockServer::start().await;

    // Configure mock response
    Mock::given(method("PROPFIND"))
        .and(path("/endpoint"))
        .respond_with(ResponseTemplate::new(207).set_body_string(
            "<!-- CalDAV XML response -->"
        ))
        .mount(&mock_server)
        .await;

    // Create client
    let client = CalDavClient::new(
        &mock_server.uri(),
        Some("user"),
        Some("pass"),
    )?;

    // Test functionality
    let result = client.some_operation().await?;

    // Assert expectations
    assert!(result.is_some());
    Ok(())
}
```

## Test Data

- **Secret files**: Created with `tempfile::NamedTempFile` for automatic cleanup
- **CalDAV responses**: Valid XML matching CalDAV RFC specifications
- **Credentials**: Non-functional test credentials (never real secrets)

## Debugging Failed Tests

### Enable tracing in tests

Set the `RUST_LOG` environment variable:

```bash
RUST_LOG=debug cargo test -- --nocapture
```

### Run single test with backtrace

```bash
RUST_BACKTRACE=1 cargo test test_name -- --nocapture
```

### Check mock server interactions

The `wiremock` crate provides detailed logging when tests fail, showing:

- Expected vs actual requests
- Request headers and body
- Response status and body

## Performance

Current test suite performance:

- **Unit tests**: ~1-10ms total
- **Integration tests**: ~10-50ms total
- **Full suite**: < 100ms on modern hardware

Target: Keep full test suite under 1 second for developer experience.
