# Caching and Sync Strategy

## Overview

`fred-cal` implements an intelligent caching and synchronization system that:

- **Loads data immediately** from cache on startup (API available instantly)
- **Uses incremental sync** when supported by the server (only fetches changes)
- **Falls back gracefully** to full sync when needed
- **Keeps the API responsive** during sync operations

## How Caching Works

### Cache Location

The cache is stored in platform-specific data directories following XDG standards:

- **Linux**: `~/.local/share/fred-cal/calendar_data.json`
- **macOS**: `~/Library/Application Support/fred-cal/calendar_data.json`
- **Windows**: `%APPDATA%\fred-cal\calendar_data.json`

### Cache Structure

The cache file stores:

- All calendar events (VEVENT components)
- All todos/tasks (VTODO components)
- Last sync timestamp
- Sync tokens per calendar (for incremental updates)

```json
{
  "events": [...],
  "todos": [...],
  "last_sync": "2026-01-03T19:00:00Z",
  "sync_tokens": {
    "/calendars/user/calendar1/": "sync-token-abc123",
    "/calendars/user/calendar2/": "sync-token-def456"
  }
}
```

### Startup Behavior

1. **Load cache immediately** - On startup, `fred-cal` loads the cache from disk
2. **API available instantly** - The API serves cached data while sync runs in background
3. **Initial sync** - Performs first sync (incremental if possible)
4. **Periodic sync** - Background task syncs every 15 minutes

This means your API has **zero downtime** even with large calendars!

## Sync Strategies

### 1. Initial Sync (calendar-query)

**When**:

- **Very first sync** for a calendar (no cached data exists)

**How it works**:

1. Query calendar for all VEVENT components
2. Query calendar for all VTODO components
3. Parse all returned iCalendar data
4. Store all events and todos
5. If server supports WebDAV sync, mark calendar for incremental sync next time

**Benefits**:

- ✅ **Reliable** - Works with all CalDAV servers
- ✅ **Complete** - Guarantees all data is fetched
- ✅ **Compatible** - No dependencies on advanced features

**Example log output**:

```shell
INFO Server supports WebDAV sync - using incremental updates
DEBUG First sync for Personal - using full query
DEBUG Fetched 142 VEVENTs from Personal
DEBUG Fetched 67 VTODOs from Personal
DEBUG Full sync for Personal: fetched 142 events and 67 todos
INFO Sync complete: 142 events, 67 todos (from 1 calendars)
```

### 2. Incremental Sync (WebDAV sync-collection)

**When**:

- Server supports WebDAV sync-collection protocol
- **AND** calendar has been synced at least once before

**How it works**:

**Second sync** (first incremental, no sync token yet):

1. Send `sync-collection` REPORT with `sync_token = None`
2. Server returns **all items** in the calendar + a sync token
3. Apply changes to in-memory data
4. Store sync token for next sync

**Subsequent syncs** (have sync token):

1. Send `sync-collection` REPORT with previous sync token
2. Server returns only **changes since last sync**:
   - New events/todos
   - Modified events/todos
   - Deleted events/todos
3. Apply changes to in-memory data
4. Store new sync token

**Benefits**:

- ⚡ **Very fast** - After second sync, only transfers changed items
- 📉 **Low bandwidth** - Minimal data transfer on updates
- 🎯 **Efficient** - Processes only what changed
- 🔄 **Smart** - Handles additions, modifications, and deletions

**Example log output**:

```shell
INFO Server supports WebDAV sync - using incremental updates
DEBUG Using sync_collection for Personal (subsequent sync)
INFO Incremental sync for Personal: +2 events, +1 todos, -0 deleted
DEBUG Using sync_collection for Work (subsequent sync)
INFO Incremental sync for Work: +0 events, +0 todos, -1 deleted
INFO Sync complete: 144 events, 68 todos (from 2 calendars)
```

### 3. Full Sync (calendar-query) - Non-Sync Servers

**When**:

- Server doesn't support WebDAV sync-collection
- **Every sync** on these servers (no incremental option)

**How it works**:

1. Query calendar for all VEVENT components
2. Query calendar for all VTODO components
3. Parse all returned iCalendar data
4. **Replace** all items from that calendar
5. **No sync tokens** - every sync is a full sync

**Benefits**:

- ✅ **Always works** - Compatible with all CalDAV servers
- 🔄 **Complete refresh** - Ensures data consistency
- 🛡️ **Universal compatibility** - Works even on legacy servers

**Drawback**:

- ⚠️ **Slower** - Always fetches everything, even if nothing changed

**Example log output**:

```shell
INFO Server does not support WebDAV sync - using full sync
DEBUG Using full sync for Personal (server doesn't support WebDAV sync)
DEBUG Full sync for Personal: fetched 142 events and 67 todos
INFO Sync complete: 142 events, 67 todos (from 1 calendars)
```

## Performance Characteristics

### Large Calendar Performance

For a calendar with **1000+ events**:

| Operation         | Cold Start | Incremental Sync | Full Sync      |
| ----------------- | ---------- | ---------------- | -------------- |
| **Cache Load**    | ~100ms     | N/A              | N/A            |
| **API Available** | Immediate  | Immediate        | Immediate      |
| **Sync Time**     | -          | ~2-5 seconds     | ~30-60 seconds |
| **Data Transfer** | -          | Only changes     | Full calendar  |

### Memory Usage

- **In-memory storage**: Events and todos kept in memory for fast API responses
- **Typical usage**: ~10-20 MB for 1000 events and 500 todos
- **Cache file**: Human-readable JSON, ~5-10 MB typical

## Sync Flow Diagram

```text
Startup
  ↓
Load Cache ────→ API Available (with cached data)
  ↓
Check Server Capabilities
  ├─→ Supports sync-collection?
  │   ├─→ YES: Check if calendar has been synced before
  │   │   ├─→ NO (first time): Use full query
  │   │   │   └─→ Query all VEVENTs + VTODOs
  │   │   │       └─→ Mark for incremental next time
  │   │   │
  │   │   └─→ YES (already synced): Use sync-collection
  │   │       ├─→ Have sync token?
  │   │       │   ├─→ YES: Send token
  │   │       │   │   └─→ Get only changes + new token
  │   │       │   │
  │   │       │   └─→ NO: Send null token
  │   │       │       └─→ Get all items + first token
  │   │
  │   └─→ NO: Use calendar-query (always)
  │       └─→ Query all VEVENTs + VTODOs (no tokens)
  │
  ↓
Update Cache
  ↓
API Serves Fresh Data
  ↓
Background Sync (every 15 minutes)
  └─→ Repeat from "Check Server Capabilities"
```

## Technical Details

### Sync Token Management

Sync tokens are stored per-calendar in the `sync_tokens` HashMap:

```rust
pub sync_tokens: std::collections::HashMap<String, String>
```

- **Key**: Calendar URL (e.g., `/calendars/user/personal/`)
- **Value**: Opaque sync token from server

Tokens are:

- **Saved** after successful incremental sync
- **Used** on next sync to get only changes
- **Cleared** when full sync is performed
- **Persisted** in cache file across restarts

### Handling Deletions

When an item is deleted on the server:

1. Server sends `is_deleted: true` in sync response
2. We match by `href` (typically ends with `{uid}.ics`)
3. Remove from both events and todos collections
4. Deletion count logged

### Conflict Resolution

**During incremental sync**:

- Items with same UID are **replaced** (server is source of truth)
- Old version removed, new version added
- ETag updated

**During full sync**:

- All items from that calendar are **removed**
- Fresh items from server are **added**
- Ensures complete consistency

## Server Compatibility

### Servers with WebDAV sync-collection

✅ **Incremental sync available**:

- Apple iCloud Calendar
- Nextcloud
- Radicale (with sync-collection plugin)
- SOGo
- Baikal

### Servers without WebDAV sync-collection

⚠️ **Full sync only**:

- Google Calendar (CalDAV)
- Some legacy CalDAV servers

The system automatically detects and adapts!

## Cache Management

### Manual Cache Operations

**Clear cache** (forces full sync on next startup):

```bash
rm ~/.local/share/fred-cal/calendar_data.json  # Linux
rm ~/Library/Application\ Support/fred-cal/calendar_data.json  # macOS
```

**Inspect cache**:

```bash
cat ~/.local/share/fred-cal/calendar_data.json | jq .
```

**Check cache size**:

```bash
du -h ~/.local/share/fred-cal/
```

### Cache Validation

The cache is validated on load:

- ✅ Valid JSON → Load successful
- ❌ Invalid JSON → Start fresh
- ❌ Missing file → Start fresh

No risk of corrupted data causing crashes!

## Best Practices

### For Small Calendars (<100 items)

- Full sync is fast enough (~1-2 seconds)
- Incremental sync still beneficial for mobile/slow connections

### For Large Calendars (1000+ items)

- **Critical**: WebDAV sync-collection support is highly beneficial
- **First sync**: Slow (fetches everything) - one-time, ~30-60 seconds
- **Second sync**: Also slow (establishes baseline + gets token) - ~30 seconds
- **Third+ syncs**: Very fast (only changes) - ~2-5 seconds - **90%+ improvement**
- Without sync-collection support, every sync is slow (~30-60 seconds)

### For Multiple Calendars

- Each calendar synced independently
- One slow calendar doesn't block others
- Errors in one calendar don't affect others

## Troubleshooting

### "Sync taking too long"

1. Check if server supports WebDAV sync: Look for "using sync_collection" in logs
2. If using full sync, first sync will be slow (expected)
3. Check network latency to CalDAV server
4. Consider sync interval (default 15 minutes) or increase it for large calendars

### "API serving stale data"

1. Check last_sync timestamp in API responses
2. Verify background sync is running (check logs)
3. Manually trigger sync by restarting service

### "Cache file too large"

- Normal for large calendars
- Consider archiving old completed todos
- JSON is human-readable but verbose (future: could use binary format)

## Future Enhancements

Potential improvements:

1. **Binary cache format** - Faster load, smaller file
2. **Partial cache loading** - Load only recent events on startup
3. **Configurable sync interval** - Per-calendar or global
4. **Compression** - gzip cache file
5. **SQLite backend** - Better for very large calendars
6. **Multi-level cache** - Recent items in memory, old items on disk

## Configuration

### Custom Port

By default, the API server listens on port 3000. You can customize this:

```bash
# Command line argument
fred-cal --port 8080 \
  --caldav-server "https://caldav.example.com" \
  --username "user@example.com" \
  --password "your-password"

# Environment variable
export API_PORT=8080
fred-cal --caldav-server "..." --username "..." --password "..."
```

The server will then be available at `http://0.0.0.0:8080` (or your chosen port).

## Monitoring

Check sync health via API:

```bash
curl http://localhost:3000/api/health
# Or if using custom port:
curl http://localhost:8080/api/health
```

Response includes `last_sync` timestamp:

```json
{
  "status": "ok",
  "timestamp": "2026-01-03T19:26:30Z"
}
```

All endpoints include `last_sync` in their responses for cache freshness verification.
