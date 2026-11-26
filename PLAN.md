# Songren — Rewrite Plan

## Architecture Overview
- **Runtime**: Native Rust binary (`axum` + `tokio` + `tracing`) running as `songren.service` via `systemd` (as `root`).
- **Packaging & Self-Update**: Distributed as a GPG-signed `.deb` (`cargo-deb`) via a static APT repo on GitHub Pages, upgraded via `unattended-upgrades` or `systemd-run`.
- **Containers**: Single Docker containers (no Docker Compose) attached to a shared bridge network (`songren`) and addressed by **container IP** discovered after startup.
- **Reverse Proxy**: Nginx (per-service config files + `nginx -t` validation before reload).
- **Static Sites**: Git repositories cloned/pulled to disk and served by Nginx.
- **DNS**: Local `dnsmasq` config snippets + Name.com REST API.
- **Concurrency**: Single global lock (`tokio::sync::Mutex<()>`) serializing sync/update operations.

---

## Implementation Phases

### Phase 1: Config, State & Core Engine
- Define the declarative service schema:
  - `[container]` — image, env, volumes, command, internal port, healthcheck/readiness, `stop_first` update strategy (`false` = zero-downtime blue/green, `true` = stop-first recreate).
  - `[git]` — repo URL, branch, target subpath for static file hosting.
  - `[nginx]` — domain(s), upstream port / static root, extra directives / websocket support.
  - `[dnsmasq]` — `A`, `AAAA`, `CNAME` records.
  - `[namecom]` — `A`, `AAAA`, `CNAME` records.
- Strict service-name validation (`[a-z0-9_-]+`) to prevent path traversal and invalid Docker/Nginx identifiers.
- Desired-vs-applied state diffing (additions, updates, teardowns) with automatic creation of working/state directories.
- Single global concurrency lock.
- Unit tests for parsing, validation, and diffing.

### Phase 2: Container Lifecycle & Endpoint Discovery
- Single-container management (`docker` CLI) with:
  - **Zero-downtime** (`stop_first = false`): pull image → run new slot (`<service>-a` / `<service>-b`) → wait for readiness → inspect container IP → update Nginx → stop & remove old slot.
  - **Recreate** (`stop_first = true`): pull image → stop & remove old container → run new container → inspect container IP → update Nginx.
- Abort cleanly on failure without tearing down the currently running container or overwriting applied state.

### Phase 3: Static Sites, Nginx & DNS Modules
- **Git static sites**: Clone, fast-forward/reset pull, and cleanup.
- **Nginx**: Generate/remove config, run `nginx -t` before `systemctl reload nginx`, and only reload on change.
- **`dnsmasq`**: Write/remove per-service config snippets and reload/restart `dnsmasq` only on change.
- **Name.com**: Paginated record listing, graceful error on missing credentials, and mocked HTTP tests.

### Phase 4: HTTP API & Self-Update
- `axum` HTTP server with bearer-token auth and `tracing` logs:
  - `POST /sync` — Reconcile all added, modified, and removed services.
  - `POST /update/:service` — Force-pull `:latest` or `git pull` a specific service.
  - `POST /self-update` — Trigger `apt-get update && apt-get install -y --only-upgrade songren` via `systemd-run`.

### Phase 5: `.deb` Packaging & Static APT Repo CI
- Add `cargo-deb` metadata to `Cargo.toml`, `songren.service` systemd unit, and `postinst` hook.
- Update GitHub Actions to build `.deb`, generate `Packages.gz` + `Release`, GPG-sign `InRelease`, and deploy to GitHub Pages.
