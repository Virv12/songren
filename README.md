# songren

A lightweight, declarative service orchestrator written in Rust for managing single-node setups with **Docker**, **Git static worktrees**, **Nginx**, **dnsmasq**, and **Name.com** DNS.

## Features

- **Single Docker containers**: Supports both stop-first updates (default, `stop_first = true`) for stateful services and zero-downtime blue/green cutovers (`stop_first = false`), using isolated bridge container IPs (no manual port allocation).
- **Git static worktrees**: Checks out static sites into clean worktree directories with no `.git` folder exposed to Nginx.
- **Nginx**: Renders raw Nginx configs with `{docker.<name>.ip}` and `{git.<name>.site}` placeholders, validates with `nginx -t` before reloading, and rolls back on failure.
- **DNS**: Manages local `dnsmasq` records and public Name.com Core v1 DNS records (storing record IDs for direct cleanup).
- **Self-updating**: Packaged as a GPG-signed `.deb` (`systemd` service) via a static APT repository on GitHub Pages.

## Installation (Debian / Ubuntu)

1. Add the repository signing key:
   ```bash
   curl -fsSL https://virv12.github.io/songren/songren.gpg | sudo tee /etc/apt/keyrings/songren.gpg > /dev/null
   ```
2. Add the APT source (`/etc/apt/sources.list.d/songren.list`):
   ```text
   deb [signed-by=/etc/apt/keyrings/songren.gpg] https://virv12.github.io/songren/ ./
   ```
3. Install and configure:
   ```bash
   sudo apt update && sudo apt install -y songren
   ```

## Configuration

### Global Config (`/etc/songren/songren.toml`)

```toml
listen_on = "127.0.0.1:8000"
docker_network = "songren"
auth_token = "your-secret-token"

services_dir = "/etc/songren/services"
state_dir = "/var/lib/songren/state"
repos_dir = "/var/lib/songren/repos"
worktrees_dir = "/var/lib/songren/worktrees"
nginx_dir = "/etc/nginx/conf.d"
dnsmasq_dir = "/etc/dnsmasq.d"

[namecom]
username = "your-username"
token = "your-api-token"
```

### Service Config (`/etc/songren/services/<name>.toml`)

Every section is optional.

```toml
[containers.web]
image = "ghcr.io/example/myapp:latest"
read_only = true
tty = true
# stop_first = false       # Opt into zero-downtime blue/green cutovers (default: true)
# stop_signal = "SIGKILL"

[containers.web.environment]
APP_ENV = "production"

[git.docs]
repo = "https://github.com/example/docs.git"
branch = "main"
# path = "/var/www/docs"   # Optional custom worktree path

[nginx]
content = """
server {
    listen 80;
    server_name app.example.com;

    location / {
        proxy_pass http://{docker.web.ip}:8080;
    }

    location /docs/ {
        alias {git.docs.site}/;
    }
}
"""

[[dnsmasq]]
type = "A"
host = "app.internal"
target = "192.168.1.10"

[[namecom]]
type = "CNAME"
domain = "example.com"
host = "app"
answer = "edge.example.org"
```

## HTTP API

All endpoints require `Authorization: Bearer <auth_token>` when `auth_token` is set:

- `POST /sync` — Reconcile all added, modified, and removed services.
- `POST /update/:service` — Force-update a specific service (e.g., pull a new `:latest` image or `git` commit).
- `POST /self-update` — Trigger `apt-get update && apt-get install -y --only-upgrade songren` via `systemd-run`.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
SONGREN_LOG=songren=debug cargo run -- ./dev-dir
```
