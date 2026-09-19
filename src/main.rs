mod api;
mod auth;
mod changelog;
mod collector;
mod db;
mod fuzzy;
mod groups;
mod import;
mod jellyfin;
mod media;
mod network;
mod playback;
mod profile;
mod recap;
mod relink;
mod state;
mod stats;
mod sync;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result, bail};
use rand::RngCore;
use tokio::sync::Notify;

use state::{AppState, CollectorStatus, JfConfig, Settings, Tasks};

const USAGE: &str = "finstats — playback statistics for Jellyfin

USAGE:
    finstats                            Run the server
    finstats import-jellystat <file>    Import a Jellystat backup (.jsonl / .json), then exit
    finstats relink                     Re-attach history to renamed items now, then exit (also runs after every sync)
    finstats --version

ENVIRONMENT:
    FINSTATS_DATA_DIR      Where the database and caches live   (default: ./data)
    FINSTATS_BIND          Address to listen on                  (default: 0.0.0.0:8080)
    FINSTATS_TRUST_PROXY   Set to 1 behind a reverse proxy to read X-Forwarded-For
    JELLYFIN_URL           Optional: skip the setup wizard…
    JELLYFIN_API_KEY       …together with an API key
    TZ                     Timezone used for \"per day\" and \"hour of day\" statistics
    RUST_LOG               Log filter                            (default: finstats=info)";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("finstats {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            return Ok(());
        }
        _ => {}
    }

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "finstats=info".into()))
        .with_target(false)
        .init();

    let data_dir = PathBuf::from(std::env::var("FINSTATS_DATA_DIR").unwrap_or_else(|_| "data".into()));
    std::fs::create_dir_all(&data_dir).with_context(|| format!("creating data directory {}", data_dir.display()))?;
    let db = db::Db::open(&data_dir.join("finstats.db"))?;

    match args.first().map(String::as_str) {
        None | Some("serve") => {}
        Some("import-jellystat") => {
            let Some(file) = args.get(1) else { bail!("usage: finstats import-jellystat <file>") };
            let started = std::time::Instant::now();
            let res = import::run(&db, std::path::Path::new(file), None)?;
            println!(
                "Imported {} plays ({} skipped as duplicates), {} users, {} libraries, {} items, {} seasons, {} episodes in {:.1}s",
                res.plays_imported, res.plays_skipped, res.users, res.libraries, res.items, res.seasons, res.episodes,
                started.elapsed().as_secs_f64()
            );
            return Ok(());
        }
        Some("relink") => {
            let r = relink::relink_orphans(&*db.conn()?)?;
            println!("Re-linked {} title plays and {} episode plays; cleaned {} names", r.titles, r.episodes, r.names_cleaned);
            return Ok(());
        }
        Some(other) => bail!("unknown command `{other}`\n\n{USAGE}"),
    }

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(8)
        .enable_all()
        .build()?
        .block_on(serve(db, data_dir))
}

async fn serve(db: db::Db, data_dir: PathBuf) -> Result<()> {
    let (config, settings, device_id) = db
        .call(|c| {
            let device_id = match db::get_setting(c, "device_id")? {
                Some(id) => id,
                None => {
                    let mut raw = [0u8; 16];
                    rand::rng().fill_bytes(&mut raw);
                    let id = hex::encode(raw);
                    db::set_setting(c, "device_id", &id)?;
                    id
                }
            };
            let stored = match (db::get_setting(c, "jellyfin_url")?, db::get_setting(c, "jellyfin_api_key")?) {
                (Some(url), Some(api_key)) => Some(JfConfig {
                    url,
                    api_key,
                    server_name: db::get_setting(c, "server_name")?.unwrap_or_else(|| "Jellyfin".into()),
                    server_version: db::get_setting(c, "server_version")?.unwrap_or_default(),
                    from_env: false,
                }),
                _ => None,
            };
            c.execute("DELETE FROM sessions WHERE expires_at <= ?1", [db::now()])?;
            network::set_manual(c, &Settings::load(c)?.home_addresses)?;
            network::reclassify(c)?;
            relink::relink_orphans(c)?;
            let settings_now = Settings::load(c)?;
            groups::detect(c, settings_now.group_window_s, None)?;
            Ok((stored, Settings::load(c)?, device_id))
        })
        .await?;

    // Environment wins over the wizard, so a compose file stays the source of truth.
    let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let config = match (env("JELLYFIN_URL"), env("JELLYFIN_API_KEY")) {
        (Some(url), Some(api_key)) => Some(JfConfig {
            url: jellyfin::normalize_url(&url)?,
            api_key,
            server_name: config.as_ref().map(|c| c.server_name.clone()).unwrap_or_else(|| "Jellyfin".into()),
            server_version: config.as_ref().map(|c| c.server_version.clone()).unwrap_or_default(),
            from_env: true,
        }),
        (Some(_), None) | (None, Some(_)) => bail!("JELLYFIN_URL and JELLYFIN_API_KEY must be set together"),
        _ => config,
    };

    let app = Arc::new(AppState {
        db,
        data_dir,
        http: jellyfin::http_client(),
        device_id,
        trust_proxy: env("FINSTATS_TRUST_PROXY").is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true")),
        config: RwLock::new(config),
        settings: RwLock::new(settings),
        tasks: Tasks::new(),
        live: RwLock::new(vec![]),
        collector: RwLock::new(CollectorStatus::default()),
        login_attempts: Mutex::new(Default::default()),
        wake: Notify::new(),
    });

    tokio::spawn(collector::run(app.clone()));
    tokio::spawn(sync::scheduler(app.clone()));
    tokio::spawn(api::prune_image_cache(app.clone()));

    let bind = env("FINSTATS_BIND").unwrap_or_else(|| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&bind).await.with_context(|| format!("listening on {bind}"))?;
    match app.config.read().unwrap().as_ref() {
        Some(c) => tracing::info!("finstats {} on http://{bind} — connected to {}", env!("CARGO_PKG_VERSION"), c.url),
        None => tracing::info!("finstats {} on http://{bind} — open it in a browser to finish setup", env!("CARGO_PKG_VERSION")),
    }

    axum::serve(listener, api::router(app).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let ctrl_c = tokio::signal::ctrl_c();
            let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
            tokio::select! { _ = ctrl_c => {}, _ = term.recv() => {} }
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}
