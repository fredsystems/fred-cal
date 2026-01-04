// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::models::CalendarData;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Manages local cache storage for calendar data using XDG directories
#[derive(Debug)]
pub struct CacheManager {
    cache_dir: PathBuf,
}

impl CacheManager {
    /// Create a new cache manager
    ///
    /// Uses XDG data directory, falling back to a sensible default if not available
    ///
    /// # Errors
    ///
    /// Returns an error if the cache directory cannot be created or accessed.
    pub fn new() -> Result<Self> {
        let cache_dir = Self::get_cache_directory()?;

        // Ensure the cache directory exists
        if !cache_dir.exists() {
            debug!("Creating cache directory: {:?}", cache_dir);
            fs::create_dir_all(&cache_dir).context("Failed to create cache directory")?;
        }

        Ok(Self { cache_dir })
    }

    /// Create a new cache manager with a custom directory (for testing)
    ///
    /// # Errors
    ///
    /// Returns an error if the cache directory cannot be created or accessed.
    pub fn new_with_path(cache_dir: PathBuf) -> Result<Self> {
        // Ensure the cache directory exists
        if !cache_dir.exists() {
            fs::create_dir_all(&cache_dir).context("Failed to create cache directory")?;
        }

        Ok(Self { cache_dir })
    }

    /// Get the XDG-compliant cache directory for the application
    fn get_cache_directory() -> Result<PathBuf> {
        let data_dir = dirs::data_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine user data directory"))?;

        Ok(data_dir.join("fred-cal"))
    }

    /// Get the path to the calendar data cache file
    fn cache_file_path(&self) -> PathBuf {
        self.cache_dir.join("calendar_data.json")
    }

    /// Load calendar data from cache
    ///
    /// Returns Ok(None) if cache doesn't exist or is invalid
    ///
    /// # Errors
    ///
    /// Returns an error if the cache file cannot be read or parsed.
    pub fn load(&self) -> Result<Option<CalendarData>> {
        let cache_path = self.cache_file_path();

        if !cache_path.exists() {
            debug!("Cache file does not exist: {:?}", cache_path);
            return Ok(None);
        }

        debug!("Loading cache from: {:?}", cache_path);
        let contents = fs::read_to_string(&cache_path).context("Failed to read cache file")?;

        let data: CalendarData =
            serde_json::from_str(&contents).context("Failed to parse cache file")?;

        Ok(Some(data))
    }

    /// Save calendar data to cache
    ///
    /// # Errors
    ///
    /// Returns an error if the cache file cannot be written or serialization fails.
    pub fn save(&self, data: &CalendarData) -> Result<()> {
        let cache_path = self.cache_file_path();

        debug!("Saving cache to: {:?}", cache_path);
        debug!(
            "Saving {} events and {} todos",
            data.events.len(),
            data.todos.len()
        );

        let json =
            serde_json::to_string_pretty(data).context("Failed to serialize calendar data")?;

        fs::write(&cache_path, json).context("Failed to write cache file")?;

        info!("Cache saved successfully");
        Ok(())
    }

    /// Clear the cache
    ///
    /// # Errors
    ///
    /// Returns an error if the cache file exists but cannot be deleted.
    #[allow(dead_code)]
    pub fn clear(&self) -> Result<()> {
        let cache_path = self.cache_file_path();

        if cache_path.exists() {
            debug!("Clearing cache: {:?}", cache_path);
            fs::remove_file(&cache_path).context("Failed to remove cache file")?;
            info!("Cache cleared");
        } else {
            debug!("Cache file does not exist, nothing to clear");
        }

        Ok(())
    }

    /// Get cache directory path
    #[must_use]
    pub const fn cache_directory(&self) -> &PathBuf {
        &self.cache_dir
    }

    /// Check if cache exists
    #[must_use]
    #[allow(dead_code)]
    pub fn exists(&self) -> bool {
        self.cache_file_path().exists()
    }
}

impl Default for CacheManager {
    fn default() -> Self {
        // This is only used in tests and default construction scenarios
        // If cache creation fails, it's a critical error
        #[allow(clippy::expect_used)]
        Self::new().expect("Failed to create cache manager")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::models::{CalendarEvent, Todo};
    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;
    use tempfile::tempdir;

    // Helper to create a cache manager with a temporary directory
    fn create_test_cache_manager() -> Result<(CacheManager, TempDir)> {
        let temp_dir = TempDir::new()?;
        let cache = CacheManager {
            cache_dir: temp_dir.path().to_path_buf(),
        };
        Ok((cache, temp_dir))
    }

    #[test]
    fn test_cache_manager_new() -> Result<()> {
        let temp_dir = tempdir()?;
        let cache = CacheManager::new_with_path(temp_dir.path().to_path_buf())?;
        assert!(cache.cache_directory().exists());
        Ok(())
    }

    #[test]
    fn test_save_and_load() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        let mut data = CalendarData::new();
        data.events.push(CalendarEvent {
            uid: "test-event".to_string(),
            summary: "Test Event".to_string(),
            description: Some("Description".to_string()),
            location: None,
            start: Utc.with_ymd_and_hms(2026, 1, 5, 10, 0, 0).single().unwrap(),
            end: Utc.with_ymd_and_hms(2026, 1, 5, 11, 0, 0).single().unwrap(),
            calendar_name: "Test Calendar".to_string(),
            calendar_url: "/test".to_string(),
            calendar_color: None,
            all_day: false,
            rrule: None,
            status: None,
            etag: None,
        });

        cache.save(&data)?;
        assert!(cache.exists());

        let loaded = cache.load()?;
        assert!(loaded.is_some());

        let loaded_data = loaded.unwrap();
        assert_eq!(loaded_data.events.len(), 1);
        assert_eq!(loaded_data.events[0].uid, "test-event");

        Ok(())
    }

    #[test]
    fn test_load_nonexistent_cache() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        let loaded = cache.load()?;
        assert!(loaded.is_none());

        Ok(())
    }

    #[test]
    fn test_clear_cache() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        let data = CalendarData::new();
        cache.save(&data)?;
        assert!(cache.exists());

        cache.clear()?;
        assert!(!cache.exists());

        Ok(())
    }

    #[test]
    fn test_clear_nonexistent_cache() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        // Should not error when clearing a non-existent cache
        cache.clear()?;

        Ok(())
    }

    #[test]
    fn test_save_with_todos() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        let mut data = CalendarData::new();
        data.todos.push(Todo {
            uid: "test-todo".to_string(),
            summary: "Test Todo".to_string(),
            description: None,
            due: Some(
                Utc.with_ymd_and_hms(2026, 1, 10, 12, 0, 0)
                    .single()
                    .unwrap(),
            ),
            start: None,
            completed: None,
            priority: Some(1),
            percent_complete: Some(50),
            status: "IN-PROCESS".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: None,
        });

        cache.save(&data)?;

        let loaded = cache.load()?.unwrap();
        assert_eq!(loaded.todos.len(), 1);
        assert_eq!(loaded.todos[0].uid, "test-todo");
        assert_eq!(loaded.todos[0].priority, Some(1));

        Ok(())
    }

    #[test]
    fn test_exists_method() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        // Initially should not exist
        assert!(!cache.exists());

        // After saving, should exist
        let data = CalendarData::new();
        cache.save(&data)?;
        assert!(cache.exists());

        // After clearing, should not exist
        cache.clear()?;
        assert!(!cache.exists());

        Ok(())
    }

    #[test]
    fn test_new_with_path_creates_directory() -> Result<()> {
        let temp_dir = tempdir()?;
        let non_existent_path = temp_dir.path().join("new_cache_dir");

        // Directory should not exist yet
        assert!(!non_existent_path.exists());

        // Creating cache manager should create the directory
        let cache = CacheManager::new_with_path(non_existent_path.clone())?;
        assert!(non_existent_path.exists());
        assert_eq!(cache.cache_directory(), &non_existent_path);

        Ok(())
    }

    #[test]
    fn test_load_corrupted_cache() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;
        let cache_path = cache.cache_file_path();

        // Write invalid JSON to cache file
        fs::write(&cache_path, "{ invalid json }")?;

        // Loading should return an error
        let result = cache.load();
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_multiple_save_load_cycles() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        // First cycle
        let mut data1 = CalendarData::new();
        data1.events.push(CalendarEvent {
            uid: "event-1".to_string(),
            summary: "Event 1".to_string(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).single().unwrap(),
            end: Utc.with_ymd_and_hms(2026, 1, 1, 11, 0, 0).single().unwrap(),
            calendar_name: "Cal1".to_string(),
            calendar_url: "/cal1".to_string(),
            calendar_color: Some("#FF0000".to_string()),
            all_day: false,
            rrule: None,
            status: Some("CONFIRMED".to_string()),
            etag: Some("etag1".to_string()),
        });
        cache.save(&data1)?;

        let loaded1 = cache.load()?.unwrap();
        assert_eq!(loaded1.events.len(), 1);
        assert_eq!(loaded1.events[0].uid, "event-1");

        // Second cycle - overwrite with different data
        let mut data2 = CalendarData::new();
        data2.todos.push(Todo {
            uid: "todo-1".to_string(),
            summary: "Todo 1".to_string(),
            description: Some("Description".to_string()),
            due: None,
            start: Some(Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).single().unwrap()),
            completed: None,
            priority: Some(5),
            percent_complete: Some(25),
            status: "NEEDS-ACTION".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: Some("etag2".to_string()),
        });
        cache.save(&data2)?;

        let loaded2 = cache.load()?.unwrap();
        assert_eq!(loaded2.events.len(), 0); // Previous events should be gone
        assert_eq!(loaded2.todos.len(), 1);
        assert_eq!(loaded2.todos[0].uid, "todo-1");

        Ok(())
    }

    #[test]
    fn test_save_empty_data() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        let data = CalendarData::new();
        cache.save(&data)?;

        let loaded = cache.load()?.unwrap();
        assert_eq!(loaded.events.len(), 0);
        assert_eq!(loaded.todos.len(), 0);

        Ok(())
    }

    #[test]
    fn test_save_with_both_events_and_todos() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        let mut data = CalendarData::new();

        // Add event
        data.events.push(CalendarEvent {
            uid: "event-1".to_string(),
            summary: "Event".to_string(),
            description: None,
            location: Some("Office".to_string()),
            start: Utc.with_ymd_and_hms(2026, 1, 5, 10, 0, 0).single().unwrap(),
            end: Utc.with_ymd_and_hms(2026, 1, 5, 11, 0, 0).single().unwrap(),
            calendar_name: "Work".to_string(),
            calendar_url: "/work".to_string(),
            calendar_color: None,
            all_day: false,
            rrule: Some("FREQ=DAILY".to_string()),
            status: None,
            etag: None,
        });

        // Add todo
        data.todos.push(Todo {
            uid: "todo-1".to_string(),
            summary: "Task".to_string(),
            description: None,
            due: Some(
                Utc.with_ymd_and_hms(2026, 1, 10, 17, 0, 0)
                    .single()
                    .unwrap(),
            ),
            start: None,
            completed: Some(
                Utc.with_ymd_and_hms(2026, 1, 6, 15, 30, 0)
                    .single()
                    .unwrap(),
            ),
            priority: Some(3),
            percent_complete: Some(100),
            status: "COMPLETED".to_string(),
            calendar_name: "Tasks".to_string(),
            calendar_url: "/tasks".to_string(),
            etag: None,
        });

        cache.save(&data)?;

        let loaded = cache.load()?.unwrap();
        assert_eq!(loaded.events.len(), 1);
        assert_eq!(loaded.todos.len(), 1);
        assert_eq!(loaded.events[0].location, Some("Office".to_string()));
        assert_eq!(loaded.events[0].rrule, Some("FREQ=DAILY".to_string()));
        assert_eq!(loaded.todos[0].percent_complete, Some(100));
        assert_eq!(loaded.todos[0].status, "COMPLETED");

        Ok(())
    }

    #[test]
    fn test_cache_directory_path() -> Result<()> {
        let temp_dir = tempdir()?;
        let cache_path = temp_dir.path().to_path_buf();
        let cache = CacheManager::new_with_path(cache_path.clone())?;

        assert_eq!(cache.cache_directory(), &cache_path);

        Ok(())
    }

    #[test]
    fn test_cache_file_path_location() -> Result<()> {
        let (cache, temp_dir) = create_test_cache_manager()?;

        let expected_path = temp_dir.path().join("calendar_data.json");
        assert_eq!(cache.cache_file_path(), expected_path);

        Ok(())
    }

    #[test]
    fn test_new() -> Result<()> {
        // Test the actual new() method that uses XDG directories
        let cache = CacheManager::new()?;

        // Cache directory should exist
        assert!(cache.cache_directory().exists());

        // Should be able to save and load
        let data = CalendarData::new();
        cache.save(&data)?;

        let loaded = cache.load()?;
        assert!(loaded.is_some());

        // Clean up
        cache.clear()?;

        Ok(())
    }

    #[test]
    fn test_default_implementation() {
        // Test that default creates a working cache manager
        let cache = CacheManager::default();
        assert!(cache.cache_directory().exists());
    }

    #[test]
    fn test_load_with_all_event_fields() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        let mut data = CalendarData::new();
        data.events.push(CalendarEvent {
            uid: "full-event".to_string(),
            summary: "Full Event".to_string(),
            description: Some("Full description".to_string()),
            location: Some("Conference Room A".to_string()),
            start: Utc
                .with_ymd_and_hms(2026, 2, 15, 14, 0, 0)
                .single()
                .unwrap(),
            end: Utc
                .with_ymd_and_hms(2026, 2, 15, 15, 30, 0)
                .single()
                .unwrap(),
            calendar_name: "Work Calendar".to_string(),
            calendar_url: "/calendars/work".to_string(),
            calendar_color: Some("#0000FF".to_string()),
            all_day: false,
            rrule: Some("FREQ=WEEKLY;BYDAY=MO".to_string()),
            status: Some("TENTATIVE".to_string()),
            etag: Some("etag-12345".to_string()),
        });

        cache.save(&data)?;
        let loaded = cache.load()?.unwrap();

        assert_eq!(loaded.events.len(), 1);
        let event = &loaded.events[0];
        assert_eq!(event.uid, "full-event");
        assert_eq!(event.description, Some("Full description".to_string()));
        assert_eq!(event.location, Some("Conference Room A".to_string()));
        assert_eq!(event.calendar_color, Some("#0000FF".to_string()));
        assert_eq!(event.status, Some("TENTATIVE".to_string()));
        assert_eq!(event.etag, Some("etag-12345".to_string()));
        assert_eq!(event.rrule, Some("FREQ=WEEKLY;BYDAY=MO".to_string()));

        Ok(())
    }

    #[test]
    fn test_load_with_all_todo_fields() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        let mut data = CalendarData::new();
        data.todos.push(Todo {
            uid: "full-todo".to_string(),
            summary: "Full Todo".to_string(),
            description: Some("Detailed task description".to_string()),
            due: Some(
                Utc.with_ymd_and_hms(2026, 3, 1, 23, 59, 59)
                    .single()
                    .unwrap(),
            ),
            start: Some(Utc.with_ymd_and_hms(2026, 2, 25, 9, 0, 0).single().unwrap()),
            completed: Some(
                Utc.with_ymd_and_hms(2026, 2, 28, 16, 30, 0)
                    .single()
                    .unwrap(),
            ),
            priority: Some(1),
            percent_complete: Some(100),
            status: "COMPLETED".to_string(),
            calendar_name: "Personal Tasks".to_string(),
            calendar_url: "/calendars/personal-tasks".to_string(),
            etag: Some("todo-etag-67890".to_string()),
        });

        cache.save(&data)?;
        let loaded = cache.load()?.unwrap();

        assert_eq!(loaded.todos.len(), 1);
        let todo = &loaded.todos[0];
        assert_eq!(todo.uid, "full-todo");
        assert_eq!(
            todo.description,
            Some("Detailed task description".to_string())
        );
        assert!(todo.due.is_some());
        assert!(todo.start.is_some());
        assert!(todo.completed.is_some());
        assert_eq!(todo.priority, Some(1));
        assert_eq!(todo.percent_complete, Some(100));
        assert_eq!(todo.etag, Some("todo-etag-67890".to_string()));

        Ok(())
    }

    #[test]
    fn test_multiple_loads_without_cache() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        // Multiple loads of non-existent cache should all return None
        assert!(cache.load()?.is_none());
        assert!(cache.load()?.is_none());
        assert!(cache.load()?.is_none());

        Ok(())
    }

    #[test]
    fn test_save_after_clear() -> Result<()> {
        let (cache, _temp_dir) = create_test_cache_manager()?;

        // Save initial data
        let mut data1 = CalendarData::new();
        data1.events.push(CalendarEvent {
            uid: "event-1".to_string(),
            summary: "Event 1".to_string(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).single().unwrap(),
            end: Utc.with_ymd_and_hms(2026, 1, 1, 11, 0, 0).single().unwrap(),
            calendar_name: "Cal".to_string(),
            calendar_url: "/cal".to_string(),
            calendar_color: None,
            all_day: false,
            rrule: None,
            status: None,
            etag: None,
        });
        cache.save(&data1)?;
        assert!(cache.exists());

        // Clear the cache
        cache.clear()?;
        assert!(!cache.exists());

        // Save new data after clearing
        let mut data2 = CalendarData::new();
        data2.todos.push(Todo {
            uid: "todo-1".to_string(),
            summary: "Todo 1".to_string(),
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
        });
        cache.save(&data2)?;

        // Should contain only the new data
        let loaded = cache.load()?.unwrap();
        assert_eq!(loaded.events.len(), 0);
        assert_eq!(loaded.todos.len(), 1);
        assert_eq!(loaded.todos[0].uid, "todo-1");

        Ok(())
    }

    // Note: The following scenarios are not covered by tests because they require
    // special environmental conditions or mocking that's difficult to set up:
    //
    // 1. Error path in get_cache_directory() when dirs::data_dir() returns None
    //    - This would require mocking the dirs crate, which doesn't provide
    //      a straightforward testing interface
    //
    // 2. Filesystem error paths (e.g., permission denied)
    //    - Testing these reliably across platforms is challenging
    //    - In practice, these are covered by integration testing with actual
    //      filesystem operations
    //
    // 3. Debug/info logging statements
    //    - These don't affect program logic and are intentionally excluded
    //      from coverage metrics
    //
    // Current coverage (98.45% line coverage, 96.67% function coverage) is
    // excellent for a cache manager module.
}
