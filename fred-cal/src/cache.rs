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

        info!(
            "Loaded cache with {} events and {} todos",
            data.events.len(),
            data.todos.len()
        );

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
    use temp_env::with_var;
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

    // #[test]
    // fn test_cache_manager_new() -> Result<()> {
    //     // Skip this test inside Nix builds
    //     if std::env::var_os("NIX_BUILD_TOP").is_some() {
    //         return Ok(());
    //     }

    //     let tmp = tempdir()?;

    //     with_var("XDG_CACHE_HOME", Some(tmp.path()), || {
    //         let cache = CacheManager::new()?;
    //         assert!(cache.cache_directory().exists());
    //         Ok(())
    //     })
    // }

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
}
