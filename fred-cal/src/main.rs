// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![deny(
    clippy::pedantic,
    //clippy::cargo,
    clippy::nursery,
    clippy::style,
    clippy::correctness,
    clippy::all,
    clippy::unwrap_used,
    clippy::expect_used
)]

#[macro_use]
extern crate tracing;

mod cli;

use anyhow::Result;
use cli::Cli;
use fast_dav_rs::CalDavClient;
use tracing_subscriber::{EnvFilter, fmt};

fn init_tracing() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new("info"))?;

    fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .init();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing()?;

    // Parse command line arguments
    let cli = Cli::parse_args();

    // Load and validate credentials
    let credentials = cli.load_credentials()?;

    info!("Connecting to CalDAV server: {}", credentials.server_url);

    let client = CalDavClient::new(
        &credentials.server_url,
        Some(&credentials.username),
        Some(&credentials.password),
    )?;

    let principal = client
        .discover_current_user_principal()
        .await?
        .ok_or_else(|| anyhow::anyhow!("no principal returned"))?;

    let homes = client.discover_calendar_home_set(&principal).await?;
    let Some(home) = homes.first() else {
        return Err(anyhow::anyhow!("missing calendar-home-set"));
    };

    for calendar in client.list_calendars(home).await? {
        info!("Calendar: {:?}", calendar.displayname);
    }

    Ok(())
}
