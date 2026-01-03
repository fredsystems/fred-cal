// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

/// `CalDAV` sync and API server
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// `CalDAV` server URL (or path to file containing URL)
    #[arg(long, env = "CALDAV_SERVER")]
    pub caldav_server: String,

    /// Username for `CalDAV` authentication (or path to file containing username)
    #[arg(long, env = "CALDAV_USERNAME")]
    pub username: String,

    /// Password for `CalDAV` authentication (or path to file containing password)
    #[arg(long, env = "CALDAV_PASSWORD")]
    pub password: String,
}

/// Credentials for `CalDAV` authentication
#[derive(Debug, Clone)]
pub struct Credentials {
    pub server_url: String,
    pub username: String,
    pub password: String,
}

impl Cli {
    /// Parse command line arguments
    #[must_use]
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Load and validate credentials from CLI arguments
    ///
    /// Each argument can be either a direct value or a path to a file.
    /// If the value exists as a file, its contents will be read and trimmed.
    /// Otherwise, the value itself will be used.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - A file path is specified but cannot be read
    /// - Credentials are invalid (empty or malformed)
    /// - The server URL doesn't start with http:// or https://
    pub fn load_credentials(&self) -> Result<Credentials> {
        let server_url =
            load_value_or_file(&self.caldav_server).context("Failed to load CalDAV server URL")?;
        let username = load_value_or_file(&self.username).context("Failed to load username")?;
        let password = load_value_or_file(&self.password).context("Failed to load password")?;

        validate_credentials(&server_url, &username, &password)?;

        Ok(Credentials {
            server_url,
            username,
            password,
        })
    }
}

/// Load a value either directly or from a file
///
/// If the value exists as a file path, read its contents.
/// Otherwise, return the value as-is.
fn load_value_or_file(value: &str) -> Result<String> {
    let path = PathBuf::from(value);

    if path.exists() && path.is_file() {
        debug!("Loading value from file: {}", value);
        let contents =
            std::fs::read_to_string(&path).context(format!("Failed to read file: {value}"))?;
        Ok(contents.trim().to_string())
    } else {
        debug!("Using value directly (not a file path)");
        Ok(value.to_string())
    }
}

/// Validate that credentials are properly formatted and non-empty
fn validate_credentials(server_url: &str, username: &str, password: &str) -> Result<()> {
    if server_url.is_empty() {
        anyhow::bail!("CalDAV server URL cannot be empty");
    }

    if username.is_empty() {
        anyhow::bail!("Username cannot be empty");
    }

    if password.is_empty() {
        anyhow::bail!("Password cannot be empty");
    }

    // Validate URL format
    if !server_url.starts_with("http://") && !server_url.starts_with("https://") {
        anyhow::bail!("CalDAV server URL must start with http:// or https://");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_value_or_file_direct_value() -> Result<()> {
        let result = load_value_or_file("direct_value")?;
        assert_eq!(result, "direct_value");
        Ok(())
    }

    #[test]
    fn test_load_value_or_file_from_file() -> Result<()> {
        let mut temp_file = NamedTempFile::new()?;
        writeln!(temp_file, "file_content")?;

        let path = temp_file
            .path()
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid path"))?;
        let result = load_value_or_file(path)?;
        assert_eq!(result, "file_content");
        Ok(())
    }

    #[test]
    fn test_load_value_or_file_trims_whitespace() -> Result<()> {
        let mut temp_file = NamedTempFile::new()?;
        writeln!(temp_file, "  trimmed_content  ")?;

        let path = temp_file
            .path()
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid path"))?;
        let result = load_value_or_file(path)?;
        assert_eq!(result, "trimmed_content");
        Ok(())
    }

    #[test]
    fn test_validate_credentials_success() {
        let result = validate_credentials(
            "https://caldav.example.com",
            "user@example.com",
            "password123",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_credentials_http_success() {
        let result = validate_credentials(
            "http://caldav.example.com",
            "user@example.com",
            "password123",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_credentials_empty_url() {
        let result = validate_credentials("", "user@example.com", "password123");
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("URL cannot be empty"));
        }
    }

    #[test]
    fn test_validate_credentials_empty_username() {
        let result = validate_credentials("https://caldav.example.com", "", "password123");
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("Username cannot be empty"));
        }
    }

    #[test]
    fn test_validate_credentials_empty_password() {
        let result = validate_credentials("https://caldav.example.com", "user@example.com", "");
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("Password cannot be empty"));
        }
    }

    #[test]
    fn test_validate_credentials_invalid_url_scheme() {
        let result = validate_credentials(
            "ftp://caldav.example.com",
            "user@example.com",
            "password123",
        );
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("must start with http"));
        }
    }

    #[test]
    fn test_validate_credentials_no_scheme() {
        let result = validate_credentials("caldav.example.com", "user@example.com", "password123");
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("must start with http"));
        }
    }
}
