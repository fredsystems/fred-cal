// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::models::CalendarData;
use crate::sync::SyncManager;
use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::{
    cors::CorsLayer,
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};

/// API response for combined calendar and todo data
#[derive(Debug, Serialize, Deserialize)]
pub struct CombinedResponse {
    pub events: Vec<crate::models::CalendarEvent>,
    pub todos: Vec<crate::models::Todo>,
    pub last_sync: DateTime<Utc>,
}

/// API response for calendar-only data
#[derive(Debug, Serialize, Deserialize)]
pub struct CalendarsResponse {
    pub events: Vec<crate::models::CalendarEvent>,
    pub last_sync: DateTime<Utc>,
}

/// API response for todos-only data
#[derive(Debug, Serialize, Deserialize)]
pub struct TodosResponse {
    pub todos: Vec<crate::models::Todo>,
    pub last_sync: DateTime<Utc>,
}

/// API error response
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub data: Arc<RwLock<CalendarData>>,
    pub sync_manager: Option<Arc<SyncManager>>,
}

/// Create the API router with all endpoints
pub fn create_router(
    data: Arc<RwLock<CalendarData>>,
    sync_manager: Option<Arc<SyncManager>>,
) -> Router {
    let state = AppState { data, sync_manager };

    Router::new()
        .route("/api/get_today", get(get_today))
        .route("/api/get_today_calendars", get(get_today_calendars))
        .route("/api/get_today_todos", get(get_today_todos))
        .route("/api/get_date_range/{range}", get(get_date_range))
        .route("/api/debug/events", get(debug_events))
        .route("/api/health", get(health_check))
        .route("/api/sync", post(trigger_sync))
        .with_state(state)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().include_headers(true))
                .on_response(DefaultOnResponse::new().include_headers(true)),
        )
        .layer(CorsLayer::permissive())
}

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "timestamp": Utc::now()
    }))
}

/// Get all events and todos for today
async fn get_today(State(state): State<AppState>) -> Result<Json<CombinedResponse>, ApiError> {
    let (start, end) = get_today_range();
    let data = state.data.read().await;

    let events: Vec<_> = data
        .events_in_range(start, end)
        .into_iter()
        .cloned()
        .collect();

    let todos: Vec<_> = data
        .todos_in_range(start, end)
        .into_iter()
        .cloned()
        .collect();

    Ok(Json(CombinedResponse {
        events,
        todos,
        last_sync: data.last_sync,
    }))
}

/// Get only calendar events for today
async fn get_today_calendars(
    State(state): State<AppState>,
) -> Result<Json<CalendarsResponse>, ApiError> {
    let (start, end) = get_today_range();
    let data = state.data.read().await;

    let events: Vec<_> = data
        .events_in_range(start, end)
        .into_iter()
        .cloned()
        .collect();

    Ok(Json(CalendarsResponse {
        events,
        last_sync: data.last_sync,
    }))
}

/// Get only todos for today
async fn get_today_todos(State(state): State<AppState>) -> Result<Json<TodosResponse>, ApiError> {
    let (start, end) = get_today_range();
    let data = state.data.read().await;

    let todos: Vec<_> = data
        .todos_in_range(start, end)
        .into_iter()
        .cloned()
        .collect();

    Ok(Json(TodosResponse {
        todos,
        last_sync: data.last_sync,
    }))
}

/// Get events and todos for a specified date range
///
/// Range formats:
/// - `"today"` - today's date
/// - `"tomorrow"` - tomorrow's date
/// - `"week"` - next 7 days
/// - `"month"` - next 30 days
/// - `"2026-01-05"` - specific date
/// - `"2026-01-05:2026-01-10"` - date range from:to
/// - `"+3d"` - 3 days from now
/// - `"-2d"` - 2 days ago
async fn get_date_range(
    State(state): State<AppState>,
    Path(range): Path<String>,
) -> Result<Json<CombinedResponse>, ApiError> {
    let (start, end) = parse_date_range(&range)?;
    let data = state.data.read().await;

    let events: Vec<_> = data
        .events_in_range(start, end)
        .into_iter()
        .cloned()
        .collect();

    let todos: Vec<_> = data
        .todos_in_range(start, end)
        .into_iter()
        .cloned()
        .collect();

    Ok(Json(CombinedResponse {
        events,
        todos,
        last_sync: data.last_sync,
    }))
}

/// Debug endpoint to show detailed event information with timezone data
async fn debug_events(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state.data.read().await;
    let now_utc = Utc::now();
    let now_local = Local::now();

    let events_debug: Vec<serde_json::Value> = data
        .events
        .iter()
        .map(|event| {
            serde_json::json!({
                "uid": event.uid,
                "summary": event.summary,
                "start_utc": event.start.to_rfc3339(),
                "end_utc": event.end.to_rfc3339(),
                "all_day": event.all_day,
                "calendar": event.calendar_name,
                "calendar_color": event.calendar_color,
                "rrule": event.rrule,
            })
        })
        .collect();

    let todos_debug: Vec<serde_json::Value> = data
        .todos
        .iter()
        .map(|todo| {
            serde_json::json!({
                "uid": todo.uid,
                "summary": todo.summary,
                "due_utc": todo.due.map(|d| d.to_rfc3339()),
                "start_utc": todo.start.map(|s| s.to_rfc3339()),
                "status": todo.status,
                "calendar": todo.calendar_name,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "current_time_utc": now_utc.to_rfc3339(),
        "current_time_local": now_local.to_rfc3339(),
        "local_timezone_offset": now_local.offset().to_string(),
        "total_events": events_debug.len(),
        "total_todos": todos_debug.len(),
        "events": events_debug,
        "todos": todos_debug,
        "last_sync": data.last_sync.to_rfc3339(),
    })))
}

/// Get the date range for "today" (midnight to midnight in local timezone, converted to UTC)
///
/// This ensures that "today" is based on the user's local timezone, not UTC.
/// For example, if it's 11pm PST (7am UTC next day), this will still return
/// events for "today" in PST, not tomorrow's UTC date.
fn get_today_range() -> (DateTime<Utc>, DateTime<Utc>) {
    let now = Local::now();
    // SAFETY: 0:0:0 is always a valid time
    let start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap_or_else(|| now.naive_local());
    let end = start + Duration::days(1);

    // Convert local times to UTC for querying
    // SAFETY: start and end are valid naive datetimes constructed from current date
    // This should never fail in practice, but we handle the Option to satisfy clippy
    let start_utc = Local
        .from_local_datetime(&start)
        .earliest()
        .unwrap_or_else(|| {
            // This should never happen with valid date/time values
            panic!("Failed to convert start time to local timezone")
        })
        .with_timezone(&Utc);
    let end_utc = Local
        .from_local_datetime(&end)
        .earliest()
        .unwrap_or_else(|| {
            // This should never happen with valid date/time values
            panic!("Failed to convert end time to local timezone")
        })
        .with_timezone(&Utc);

    (start_utc, end_utc)
}

/// Parse a date range string into start and end `DateTime`s
///
/// All date ranges are calculated based on the local timezone and then converted to UTC.
fn parse_date_range(range: &str) -> Result<(DateTime<Utc>, DateTime<Utc>), ApiError> {
    let now = Local::now();
    // SAFETY: 0:0:0 is always a valid time
    let today_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap_or_else(|| now.naive_local());

    match range {
        "today" => {
            let start = Local
                .from_local_datetime(&today_start)
                .earliest()
                .ok_or_else(|| {
                    ApiError::InvalidDateRange(
                        "Failed to convert today to local timezone".to_string(),
                    )
                })?
                .with_timezone(&Utc);
            let end = start + Duration::days(1);
            Ok((start, end))
        }
        "tomorrow" => {
            let tomorrow_start = today_start + Duration::days(1);
            let start = Local
                .from_local_datetime(&tomorrow_start)
                .earliest()
                .ok_or_else(|| {
                    ApiError::InvalidDateRange(
                        "Failed to convert tomorrow to local timezone".to_string(),
                    )
                })?
                .with_timezone(&Utc);
            let end = start + Duration::days(1);
            Ok((start, end))
        }
        "week" => {
            let start = Local
                .from_local_datetime(&today_start)
                .earliest()
                .ok_or_else(|| {
                    ApiError::InvalidDateRange(
                        "Failed to convert week start to local timezone".to_string(),
                    )
                })?
                .with_timezone(&Utc);
            let end = start + Duration::days(7);
            Ok((start, end))
        }
        "month" => {
            let start = Local
                .from_local_datetime(&today_start)
                .earliest()
                .ok_or_else(|| {
                    ApiError::InvalidDateRange(
                        "Failed to convert month start to local timezone".to_string(),
                    )
                })?
                .with_timezone(&Utc);
            let end = start + Duration::days(30);
            Ok((start, end))
        }
        range_str if range_str.contains(':') => {
            // Parse "start:end" format
            let parts: Vec<&str> = range_str.split(':').collect();
            if parts.len() != 2 {
                return Err(ApiError::InvalidDateRange(
                    "Range must be in format 'start:end'".to_string(),
                ));
            }

            let start = parse_single_date(parts[0])?;
            let end = parse_single_date(parts[1])?;

            Ok((start, end))
        }
        range_str if range_str.starts_with('+') || range_str.starts_with('-') => {
            // Parse relative date like "+3d" or "-2d"
            parse_relative_date(range_str, now)
        }
        date_str => {
            // Parse single date
            let start = parse_single_date(date_str)?;
            let end = start + Duration::days(1);
            Ok((start, end))
        }
    }
}

/// Parse a single date string (YYYY-MM-DD format)
///
/// Dates are interpreted as midnight in the local timezone, then converted to UTC.
fn parse_single_date(date_str: &str) -> Result<DateTime<Utc>, ApiError> {
    let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .map_err(|_| ApiError::InvalidDateRange(format!("Invalid date format: {date_str}")))?;

    let datetime = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ApiError::InvalidDateRange("Invalid time".to_string()))?;

    // Interpret as local time and convert to UTC
    let local_dt = Local
        .from_local_datetime(&datetime)
        .earliest()
        .ok_or_else(|| {
            ApiError::InvalidDateRange("Failed to convert date to local timezone".to_string())
        })?;
    Ok(local_dt.with_timezone(&Utc))
}

/// Parse a relative date like "+3d" or "-2d"
///
/// Relative dates are calculated from the current local time.
fn parse_relative_date(
    range_str: &str,
    base: DateTime<Local>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), ApiError> {
    let is_negative = range_str.starts_with('-');
    let num_str = &range_str[1..range_str.len() - 1];
    let unit = range_str
        .chars()
        .last()
        .ok_or_else(|| ApiError::InvalidDateRange("Empty range string".to_string()))?;

    let num: i64 = num_str.parse().map_err(|_| {
        ApiError::InvalidDateRange(format!("Invalid number in relative date: {range_str}"))
    })?;

    let num = if is_negative { -num } else { num };

    let start = match unit {
        'd' => base + Duration::days(num),
        'w' => base + Duration::weeks(num),
        _ => {
            return Err(ApiError::InvalidDateRange(format!(
                "Invalid unit '{unit}'. Use 'd' for days or 'w' for weeks"
            )));
        }
    };

    let end = start + Duration::days(1);
    // Convert to UTC
    Ok((start.with_timezone(&Utc), end.with_timezone(&Utc)))
}

/// API error type
#[derive(Debug)]
enum ApiError {
    InvalidDateRange(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::InvalidDateRange(msg) => (StatusCode::BAD_REQUEST, msg),
        };

        let body = Json(ErrorResponse { error: message });

        (status, body).into_response()
    }
}

/// Trigger a manual sync
async fn trigger_sync(State(state): State<AppState>) -> Response {
    match &state.sync_manager {
        Some(sync_manager) => {
            info!("Manual sync triggered via API");
            match sync_manager.sync().await {
                Ok(()) => {
                    let data = state.data.read().await;
                    Json(serde_json::json!({
                        "status": "success",
                        "message": "Sync completed successfully",
                        "events": data.events.len(),
                        "todos": data.todos.len(),
                        "last_sync": data.last_sync
                    }))
                    .into_response()
                }
                Err(e) => {
                    error!("Manual sync failed: {:?}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: format!("Sync failed: {}", e),
                        }),
                    )
                        .into_response()
                }
            }
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Sync manager not available".to_string(),
            }),
        )
            .into_response(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone};

    #[test]
    fn test_get_today_range() {
        let (start, end) = get_today_range();
        assert!(end > start);
        assert_eq!((end - start).num_days(), 1);
    }

    #[test]
    fn test_parse_date_range_today() {
        let result = parse_date_range("today");
        assert!(result.is_ok());
        let (start, end) = result.unwrap();
        assert_eq!((end - start).num_days(), 1);
    }

    #[test]
    fn test_parse_date_range_tomorrow() {
        let result = parse_date_range("tomorrow");
        assert!(result.is_ok());
        let (start, end) = result.unwrap();
        assert_eq!((end - start).num_days(), 1);
    }

    #[test]
    fn test_parse_date_range_week() {
        let result = parse_date_range("week");
        assert!(result.is_ok());
        let (start, end) = result.unwrap();
        assert_eq!((end - start).num_days(), 7);
    }

    #[test]
    fn test_parse_date_range_month() {
        let result = parse_date_range("month");
        assert!(result.is_ok());
        let (start, end) = result.unwrap();
        assert_eq!((end - start).num_days(), 30);
    }

    #[test]
    fn test_parse_single_date() {
        let result = parse_single_date("2026-01-05");
        assert!(result.is_ok());
        let date = result.unwrap();
        assert_eq!(date.date_naive().year(), 2026);
        assert_eq!(date.date_naive().month(), 1);
        assert_eq!(date.date_naive().day(), 5);
    }

    #[test]
    fn test_parse_date_range_specific() {
        let result = parse_date_range("2026-01-05");
        assert!(result.is_ok());
        let (start, end) = result.unwrap();
        assert_eq!(start.date_naive().year(), 2026);
        assert_eq!(start.date_naive().month(), 1);
        assert_eq!(start.date_naive().day(), 5);
        assert_eq!((end - start).num_days(), 1);
    }

    #[test]
    fn test_parse_date_range_with_range() {
        let result = parse_date_range("2026-01-05:2026-01-10");
        assert!(result.is_ok());
        let (start, end) = result.unwrap();
        assert_eq!(start.date_naive().year(), 2026);
        assert_eq!(start.date_naive().month(), 1);
        assert_eq!(start.date_naive().day(), 5);
        assert_eq!(end.date_naive().year(), 2026);
        assert_eq!(end.date_naive().month(), 1);
        assert_eq!(end.date_naive().day(), 10);
    }

    #[test]
    fn test_parse_relative_date_positive() {
        let base_opt = Local.with_ymd_and_hms(2026, 1, 5, 12, 0, 0).single();
        assert!(base_opt.is_some());
        let base = base_opt.unwrap();
        let result = parse_relative_date("+3d", base);
        assert!(result.is_ok());
        let (start, _) = result.unwrap();
        assert_eq!(start.date_naive().day(), 8);
    }

    #[test]
    fn test_parse_relative_date_negative() {
        let base_opt = Local.with_ymd_and_hms(2026, 1, 5, 12, 0, 0).single();
        assert!(base_opt.is_some());
        let base = base_opt.unwrap();
        let result = parse_relative_date("-2d", base);
        assert!(result.is_ok());
        let (start, _) = result.unwrap();
        assert_eq!(start.date_naive().day(), 3);
    }

    #[test]
    fn test_parse_relative_date_weeks() {
        let base_opt = Local.with_ymd_and_hms(2026, 1, 5, 12, 0, 0).single();
        assert!(base_opt.is_some());
        let base = base_opt.unwrap();
        let result = parse_relative_date("+1w", base);
        assert!(result.is_ok());
        let (start, _) = result.unwrap();
        assert_eq!(start.date_naive().day(), 12);
    }

    #[test]
    fn test_parse_invalid_date_range() {
        assert!(parse_date_range("invalid").is_err());
        assert!(parse_date_range("2026-13-01").is_err());
        assert!(parse_date_range("+3x").is_err());
    }

    // Note: Full endpoint testing is done in integration tests
    // These basic tests verify the router configuration
}
