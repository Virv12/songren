mod config;
mod dnsmasq;
mod docker;
mod git;
mod namecom;
mod nginx;
mod reconcile;
mod state;

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use axum::{
    extract::{Path, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::post,
    Router,
};
use tokio::{net::TcpListener, process::Command, sync::Mutex};
use tracing::{error, info, warn};
use tracing_subscriber::{filter::Targets, layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::{validate_service_name, GlobalConfig};

struct AppState {
    global: GlobalConfig,
    lock: Mutex<()>,
}

fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/sync", post(handle_sync))
        .route("/update/:service", post(handle_update))
        .route("/self-update", post(handle_self_update))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state)
}

async fn require_auth(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(expected) = &state.global.auth_token {
        let provided = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));

        if provided != Some(expected.as_str()) {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }

    Ok(next.run(req).await)
}

async fn handle_sync(State(state): State<Arc<AppState>>) -> StatusCode {
    let _guard = state.lock.lock().await;
    match reconcile::sync_all(&state.global).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(err) => {
            error!(error = ?err, "Sync failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn handle_update(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> StatusCode {
    if validate_service_name(&service).is_err() {
        return StatusCode::BAD_REQUEST;
    }

    let _guard = state.lock.lock().await;
    match reconcile::reconcile_service(&state.global, &service, true).await {
        Ok(true) => StatusCode::NO_CONTENT,
        Ok(false) => StatusCode::NOT_FOUND,
        Err(err) => {
            error!(service, error = ?err, "Service update failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn handle_self_update(State(state): State<Arc<AppState>>) -> StatusCode {
    let _guard = state.lock.lock().await;
    info!("Scheduling songren self-update via systemd-run");

    let output = Command::new("systemd-run")
        .args([
            "--unit=songren-upgrade",
            "--collect",
            "/bin/sh",
            "-c",
            "apt-get update && apt-get install -y --only-upgrade songren",
        ])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => StatusCode::ACCEPTED,
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            error!(stderr = %stderr.trim(), "systemd-run failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
        Err(err) => {
            error!(error = ?err, "Failed to invoke systemd-run");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter: Targets = std::env::var("SONGREN_LOG")
        .unwrap_or_else(|_| "songren=info".into())
        .parse()
        .unwrap_or_else(|_| Targets::new().with_default(tracing::Level::INFO));

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(filter)
        .init();

    let mut args = std::env::args().skip(1);
    let Some(base_dir) = args.next() else {
        bail!("Usage: songren <dir>");
    };
    if args.next().is_some() {
        bail!("Usage: songren <dir>");
    }

    let global = state::load_global_config(&base_dir)?;
    state::ensure_dirs(&global)?;

    if let Err(err) = reconcile::refresh_ips(&global).await {
        warn!(error = ?err, "Startup IP refresh failed");
    }

    if global.auth_token.is_none() {
        warn!("auth_token is not set in songren.toml; HTTP endpoints are unauthenticated");
    }

    let listen_on = global.listen_on.clone();
    let state = Arc::new(AppState {
        global,
        lock: Mutex::new(()),
    });

    let listener = TcpListener::bind(&listen_on)
        .await
        .with_context(|| format!("Failed to bind to {listen_on}"))?;
    info!(%listen_on, "Songren listening");

    axum::serve(listener, build_router(state)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    async fn spawn_test_server(auth_token: Option<&str>) -> (String, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let token_line = match auth_token {
            Some(t) => format!("auth_token = \"{t}\"\n"),
            None => String::new(),
        };
        std::fs::write(
            tmp.path().join("songren.toml"),
            format!("listen_on = \"127.0.0.1:0\"\n{token_line}"),
        )
        .unwrap();

        let global = state::load_global_config(tmp.path()).unwrap();
        state::ensure_dirs(&global).unwrap();
        let state = Arc::new(AppState {
            global,
            lock: Mutex::new(()),
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, build_router(state)).await.unwrap();
        });

        (format!("http://{addr}"), tmp)
    }

    #[tokio::test]
    async fn enforces_bearer_auth_and_validates_routes() {
        let (base, _tmp) = spawn_test_server(Some("secret-token")).await;
        let client = reqwest::Client::new();

        // Missing auth -> 401
        let res = client.post(format!("{base}/sync")).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Wrong auth -> 401
        let res = client
            .post(format!("{base}/sync"))
            .bearer_auth("wrong")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Valid auth on empty dir -> 204
        let res = client
            .post(format!("{base}/sync"))
            .bearer_auth("secret-token")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        // Invalid service name -> 400
        let res = client
            .post(format!("{base}/update/-bad"))
            .bearer_auth("secret-token")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        // Unknown service -> 404
        let res = client
            .post(format!("{base}/update/missing"))
            .bearer_auth("secret-token")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}
