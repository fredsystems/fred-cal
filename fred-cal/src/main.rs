// Copyright (C) 2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use anyhow::Result;
use fast_dav_rs::CalDavClient;
use fred_cal::api::create_router;
use fred_cal::cache::CacheManager;
use fred_cal::cli::Cli;
use fred_cal::sync::SyncManager;
use std::sync::Arc;
use tracing_subscriber::{EnvFilter, fmt};

#[macro_use]
extern crate tracing;

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
    // Initialize rustls crypto provider (needed for reqwest with rustls)
    let _ = rustls::crypto::ring::default_provider().install_default();

    init_tracing()?;

    // Parse command line arguments
    let cli = Cli::parse_args();
    let port = cli.port;
    let diagnose_colors = cli.diagnose_colors;

    // Load and validate credentials
    let credentials = cli.load_credentials()?;

    info!("Starting fred-cal CalDAV sync service");
    info!("CalDAV server: {}", credentials.server_url);

    // Initialize cache manager
    let cache = CacheManager::new()?;
    info!("Cache directory: {:?}", cache.cache_directory());

    // Create CalDAV client
    let client = CalDavClient::new(
        &credentials.server_url,
        Some(&credentials.username),
        Some(&credentials.password),
    )?;

    // Create sync manager
    let sync_manager = Arc::new(SyncManager::new(
        client,
        cache,
        credentials.server_url.clone(),
        credentials.username.clone(),
        credentials.password.clone(),
    )?);

    // If diagnostic mode is enabled, run color diagnostics and exit
    if diagnose_colors {
        info!("Running calendar color diagnostics...");

        // Create a new client for diagnostic purposes
        let diag_client = CalDavClient::new(
            &credentials.server_url,
            Some(&credentials.username),
            Some(&credentials.password),
        )?;

        // First, discover calendars
        let principal = diag_client
            .discover_current_user_principal()
            .await?
            .ok_or_else(|| anyhow::anyhow!("No principal returned"))?;
        let homes = diag_client.discover_calendar_home_set(&principal).await?;
        let home = homes
            .first()
            .ok_or_else(|| anyhow::anyhow!("Missing calendar-home-set"))?;
        let calendars = diag_client.list_calendars(home).await?;

        info!("Found {} calendars to diagnose", calendars.len());

        for calendar in &calendars {
            let calendar_name = calendar
                .displayname
                .clone()
                .unwrap_or_else(|| "Unnamed".to_string());
            info!("\n=== Checking calendar: {} ===", calendar_name);
            info!("URL: {}", calendar.href);
            info!("Color from fast-dav-rs: {:?}", calendar.color);

            // Run custom PROPFIND diagnostic
            if let Err(e) = sync_manager
                .diagnose_calendar_color(
                    &calendar.href,
                    &credentials.server_url,
                    &credentials.username,
                    &credentials.password,
                )
                .await
            {
                error!("Diagnostic failed for {}: {:?}", calendar_name, e);
            }
        }

        info!("\n=== Diagnostic complete - exiting ===");
        return Ok(());
    }

    // Perform initial sync
    info!("Performing initial sync...");
    sync_manager.sync().await?;
    info!("Initial sync complete");

    // Get reference to calendar data for API
    let calendar_data = sync_manager.data();

    // Start background sync task (every 15 minutes)
    let sync_handle = {
        let sync_manager = Arc::clone(&sync_manager);
        tokio::spawn(async move {
            sync_manager.start_periodic_sync(15).await;
        })
    };

    // Create and start web server
    let app = create_router(calendar_data);
    let bind_addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;

    info!("API server listening on http://{}", bind_addr);
    info!("Available endpoints:");
    info!("  - GET /api/health");
    info!("  - GET /api/get_today");
    info!("  - GET /api/get_today_calendars");
    info!("  - GET /api/get_today_todos");
    info!("  - GET /api/get_date_range/:range");
    info!("  - GET /api/debug/events (diagnostic endpoint)");

    // Run the server
    axum::serve(listener, app).await?;

    // Wait for background sync to complete (it won't, it runs forever)
    sync_handle.await?;

    Ok(())
}
