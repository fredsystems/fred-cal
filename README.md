# fred-cal

A CalDAV sync and API server that provides a JSON REST API for accessing calendar events and todos.

> **Note**: The CalDAV sync infrastructure is complete and working. Calendar discovery and sync scheduling is functional. The next step is implementing full iCalendar (VEVENT/VTODO) parsing to populate event and todo data. Currently, the API returns empty data sets but all endpoints are operational and tested.

## Features

- 🔄 **CalDAV Synchronization**: Connects to CalDAV servers (iCloud, Nextcloud, etc.) and syncs calendar data
- 💾 **XDG-Compliant Caching**: Stores calendar data locally using XDG directory standards for fast access
- 🔄 **Background Sync**: Automatically syncs with CalDAV server on a configurable interval
- 🌐 **REST API**: Provides JSON endpoints for accessing calendar and todo data
- 🔒 **Security-First**: No secrets in code or config files - credentials loaded from files or environment variables
- ✅ **Fully Tested**: Comprehensive test coverage with unit and integration tests
- 🚀 **Async/Performance**: Built on Tokio for high-performance async I/O

## Quick Start

### Installation

```bash
cargo build --release
```

### Usage

```bash
# Using file-based credentials (recommended)
./target/release/fred-cal \
  --caldav-server /run/secrets/email/icloud/caldav_server \
  --username /run/secrets/email/icloud/address \
  --password /run/secrets/email/icloud/password

# Using direct values
./target/release/fred-cal \
  --caldav-server "https://caldav.example.com" \
  --username "user@example.com" \
  --password "your-password"

# Using environment variables
export CALDAV_SERVER="https://caldav.example.com"
export CALDAV_USERNAME="user@example.com"
export CALDAV_PASSWORD="your-password"
./target/release/fred-cal

# Custom port (default is 3000)
./target/release/fred-cal \
  --caldav-server "https://caldav.example.com" \
  --username "user@example.com" \
  --password "your-password" \
  --port 8080
```

The server will:

1. Perform an initial sync with your CalDAV server
2. Cache the data locally in `~/.local/share/fred-cal/`
3. Start the API server on `http://0.0.0.0:3000`
4. Sync with CalDAV server every 15 minutes in the background

## API Endpoints

All endpoints return JSON responses.

### Health Check

```bash
GET /api/health
```

Returns server health status and current timestamp.

### Get Today's Events and Todos

```bash
GET /api/get_today
```

Returns all calendar events and todos for today.

**Response:**

```json
{
  "events": [...],
  "todos": [...],
  "last_sync": "2026-01-03T18:30:00Z"
}
```

### Get Today's Calendar Events Only

```bash
GET /api/get_today_calendars
```

Returns only calendar events for today (no todos).

### Get Today's Todos Only

```bash
GET /api/get_today_todos
```

Returns only todos for today (no calendar events).

### Get Date Range

```bash
GET /api/get_date_range/:range
```

Returns events and todos for a specified date range.

**Range Formats:**

- `today` - Today's date
- `tomorrow` - Tomorrow's date
- `week` - Next 7 days from today
- `month` - Next 30 days from today
- `2026-01-05` - Specific date (returns that day)
- `2026-01-05:2026-01-10` - Date range from start to end
- `+3d` - 3 days from now
- `-2d` - 2 days ago
- `+1w` - 1 week from now

**Examples:**

```bash
# Get this week's events
curl http://localhost:3000/api/get_date_range/week

# Get events for a specific date
curl http://localhost:3000/api/get_date_range/2026-01-15

# Get events for next 3 days
curl http://localhost:3000/api/get_date_range/+3d
```

## Data Models

### CalendarEvent

```json
{
  "uid": "unique-event-id",
  "summary": "Event Title",
  "description": "Event description",
  "location": "Event location",
  "start": "2026-01-05T10:00:00Z",
  "end": "2026-01-05T11:00:00Z",
  "calendar_name": "Personal",
  "calendar_url": "/calendars/user/personal/",
  "all_day": false,
  "rrule": "FREQ=WEEKLY;BYDAY=MO",
  "status": "CONFIRMED",
  "etag": "..."
}
```

### Todo

```json
{
  "uid": "unique-todo-id",
  "summary": "Todo Title",
  "description": "Todo description",
  "due": "2026-01-10T12:00:00Z",
  "start": "2026-01-05T09:00:00Z",
  "completed": null,
  "priority": 1,
  "percent_complete": 50,
  "status": "IN-PROCESS",
  "calendar_name": "Tasks",
  "calendar_url": "/calendars/user/tasks/",
  "etag": "..."
}
```

## Configuration

### Command Line Options

- `--caldav-server <URL>` - CalDAV server URL (or path to file containing URL)
- `--username <USERNAME>` - Username for authentication (or path to file)
- `--password <PASSWORD>` - Password for authentication (or path to file)

### Environment Variables

- `CALDAV_SERVER` - CalDAV server URL
- `CALDAV_USERNAME` - Username
- `CALDAV_PASSWORD` - Password

### Data Storage

Calendar data is cached in the XDG data directory:

- Linux: `~/.local/share/fred-cal/`
- macOS: `~/Library/Application Support/fred-cal/`
- Windows: `%APPDATA%\fred-cal\`

## Development

### Prerequisites

- Rust 1.70+ (edition 2024)
- Cargo

### Building

```bash
cargo build
```

### Running Tests

```bash
# Run all tests
cargo test

# Run with verbose output
cargo test -- --nocapture

# Run only unit tests
cargo test --bins

# Run only integration tests
cargo test --test integration_tests
```

### Code Quality

```bash
# Run clippy (linter)
cargo clippy --all-targets -- -D warnings

# Format code
cargo fmt

# Check formatting
cargo fmt -- --check
```

## Architecture

```text
fred-cal/
├── src/
│   ├── main.rs          # Application entry point
│   ├── cli.rs           # Command line argument parsing
│   ├── models.rs        # Data models (CalendarEvent, Todo, etc.)
│   ├── cache.rs         # XDG-compliant cache management
│   ├── sync.rs          # CalDAV sync manager
│   └── api.rs           # REST API endpoints
├── tests/
│   └── integration_tests.rs  # Integration tests with mock CalDAV server
└── Cargo.toml
```

### Key Components

- **CLI Module**: Handles argument parsing and credential loading (file or direct)
- **Cache Manager**: XDG-compliant local storage for calendar data
- **Sync Manager**: Manages CalDAV synchronization with background updates
- **API Server**: Axum-based REST API with JSON responses
- **Models**: Type-safe data structures for events and todos

## Security Best Practices

1. **Never hardcode credentials**: Use file paths or environment variables
2. **Restrict file permissions**:

   ```bash
   chmod 600 /run/secrets/email/icloud/*
   ```

3. **Use HTTPS**: Always connect to CalDAV servers over HTTPS
4. **Don't commit secrets**: Never commit secret files to version control

## Testing

The project includes comprehensive test coverage:

- **33 unit tests** covering core functionality
- **5 integration tests** with mock CalDAV server
- **0 warnings** with strict clippy lints enabled
- **No unwrap/expect** in production code paths

See [TESTING.md](TESTING.md) for detailed testing documentation.

## License

MIT License - see [LICENSE](LICENSE) file for details.

## Contributing

Contributions are welcome! Please ensure:

1. All tests pass: `cargo test`
2. Code is formatted: `cargo fmt`
3. No clippy warnings: `cargo clippy --all-targets -- -D warnings`
4. New features include tests

## Roadmap

- [x] CalDAV connection and authentication
- [x] Local caching with XDG directories
- [x] Background sync
- [x] REST API for calendar events and todos
- [x] Comprehensive testing
- [ ] Full iCalendar parsing (VEVENT, VTODO)
- [ ] Write-back support (modify calendars/todos)
- [ ] Recurring event expansion
- [ ] WebSocket support for real-time updates
- [ ] Multi-calendar filtering
- [ ] Search functionality

## Acknowledgments

Built with:

- [axum](https://github.com/tokio-rs/axum) - Web framework
- [tokio](https://tokio.rs/) - Async runtime
- [fast-dav-rs](https://github.com/mhatzl/fast-dav-rs) - CalDAV client
- [chrono](https://github.com/chronotope/chrono) - Date/time handling
- [serde](https://serde.rs/) - Serialization framework

## Support

For issues and feature requests, please use the GitHub issue tracker.
