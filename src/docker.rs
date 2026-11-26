use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
use tokio::process::Command;
use tracing::{info, warn};

use crate::config::{AppliedContainer, ContainerConfig, Slot};

const READINESS_TIMEOUT: Duration = Duration::from_secs(30);
const READINESS_POLL: Duration = Duration::from_millis(500);

pub fn container_name(service: &str, name: &str, slot: Option<Slot>) -> String {
    match slot {
        Some(slot) => format!("{service}-{name}-{slot}"),
        None => format!("{service}-{name}"),
    }
}

pub fn build_run_args(name: &str, network: &str, cfg: &ContainerConfig) -> Vec<String> {
    let mut args = vec![
        "run".to_owned(),
        "-d".to_owned(),
        "--name".to_owned(),
        name.to_owned(),
        "--restart".to_owned(),
        cfg.restart.clone(),
        "--network".to_owned(),
        cfg.network_mode
            .clone()
            .unwrap_or_else(|| network.to_owned()),
    ];

    if let Some(user) = &cfg.user {
        args.push("--user".to_owned());
        args.push(user.clone());
    }
    if let Some(workdir) = &cfg.working_dir {
        args.push("--workdir".to_owned());
        args.push(workdir.clone());
    }
    if cfg.read_only {
        args.push("--read-only".to_owned());
    }
    if cfg.tty {
        args.push("-t".to_owned());
    }
    if let Some(signal) = &cfg.stop_signal {
        args.push("--stop-signal".to_owned());
        args.push(signal.clone());
    }
    for (k, v) in &cfg.environment {
        args.push("-e".to_owned());
        args.push(format!("{k}={v}"));
    }
    for vol in &cfg.volumes {
        args.push("-v".to_owned());
        args.push(vol.clone());
    }
    for port in &cfg.ports {
        args.push("-p".to_owned());
        args.push(port.clone());
    }
    for (k, v) in &cfg.labels {
        args.push("--label".to_owned());
        args.push(format!("{k}={v}"));
    }
    if let Some(ep) = cfg.entrypoint.first() {
        args.push("--entrypoint".to_owned());
        args.push(ep.clone());
    }

    args.push(cfg.image.clone());

    if cfg.entrypoint.len() > 1 {
        args.extend(cfg.entrypoint[1..].iter().cloned());
    }
    args.extend(cfg.command.iter().cloned());

    args
}

async fn run_docker(args: &[&str]) -> Result<String> {
    let output = Command::new("docker")
        .args(args)
        .output()
        .await
        .with_context(|| format!("Failed to execute: docker {}", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("docker {} failed: {}", args[0], stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub async fn ensure_network(network: &str) -> Result<()> {
    let inspect = Command::new("docker")
        .args(["network", "inspect", network])
        .output()
        .await
        .context("Failed to execute docker network inspect")?;

    if !inspect.status.success() {
        info!(network, "Creating isolated Docker network");
        run_docker(&[
            "network",
            "create",
            "-o",
            "com.docker.network.bridge.enable_icc=false",
            network,
        ])
        .await?;
    }
    Ok(())
}

pub async fn pull_image(image: &str) -> Result<()> {
    info!(image, "Pulling image");
    run_docker(&["pull", image]).await?;
    Ok(())
}

pub async fn remove_container(name: &str) -> Result<()> {
    let output = Command::new("docker")
        .args(["rm", "-f", name])
        .output()
        .await
        .with_context(|| format!("Failed to execute docker rm -f {name}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("No such container") {
            bail!("docker rm -f {name} failed: {}", stderr.trim());
        }
    }
    Ok(())
}

pub async fn inspect_ip(
    name: &str,
    network_mode: Option<&str>,
    default_network: &str,
) -> Result<Option<IpAddr>> {
    match network_mode {
        Some("host") => return Ok(Some(IpAddr::V4(Ipv4Addr::LOCALHOST))),
        Some("none") => return Ok(None),
        _ => {}
    }

    let net = network_mode.unwrap_or(default_network);
    let fmt = format!(
        "{{{{with index .NetworkSettings.Networks \"{net}\"}}}}{{{{.IPAddress}}}}{{{{end}}}}"
    );
    let raw = run_docker(&["inspect", "--format", &fmt, name]).await?;
    if raw.is_empty() {
        Ok(None)
    } else {
        let ip = raw
            .parse::<IpAddr>()
            .with_context(|| format!("Invalid IP address '{raw}' for container '{name}'"))?;
        Ok(Some(ip))
    }
}

pub async fn wait_ready(name: &str, timeout: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    let fmt = "{{.State.Running}} {{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}";

    loop {
        let status = run_docker(&["inspect", "--format", fmt, name]).await?;
        let mut parts = status.split_whitespace();
        let running = parts.next().unwrap_or("false");
        let health = parts.next().unwrap_or("none");

        ensure!(
            running == "true",
            "Container '{name}' is not running (status: {status})"
        );

        match health {
            "healthy" | "none" => return Ok(()),
            "unhealthy" => bail!("Container '{name}' reported unhealthy"),
            _ => {
                if tokio::time::Instant::now() >= deadline {
                    bail!("Timed out waiting for container '{name}' to become healthy (last: {health})");
                }
                tokio::time::sleep(READINESS_POLL).await;
            }
        }
    }
}

pub struct PreparedContainers {
    pub applied: BTreeMap<String, AppliedContainer>,
    pub cleanup: Vec<String>,
}

/// Pull images and start updated containers.
/// For `stop_first = false` (default), old containers are returned in `cleanup`
/// so callers can switch Nginx traffic before stopping them.
pub async fn prepare_containers(
    service: &str,
    network: &str,
    desired: &BTreeMap<String, ContainerConfig>,
    old: &BTreeMap<String, AppliedContainer>,
) -> Result<PreparedContainers> {
    if !desired.is_empty() {
        ensure_network(network).await?;
    }

    let mut applied = BTreeMap::new();
    let mut cleanup = Vec::new();

    for (name, old_state) in old {
        if !desired.contains_key(name) {
            cleanup.push(container_name(service, name, old_state.active_slot));
        }
    }

    for (name, cfg) in desired {
        pull_image(&cfg.image).await?;

        let old_entry = old.get(name);
        let old_slot = old_entry.and_then(|c| c.active_slot);
        let new_slot = if cfg.stop_first {
            None
        } else {
            Some(old_slot.map(Slot::next).unwrap_or(Slot::A))
        };
        let new_name = container_name(service, name, new_slot);

        if cfg.stop_first {
            if let Some(prev) = old_entry {
                let prev_name = container_name(service, name, prev.active_slot);
                info!(container = %prev_name, "Stopping existing container (stop_first)");
                remove_container(&prev_name).await?;
            }
        }

        // Remove any stale container occupying the target name.
        remove_container(&new_name).await?;

        let run_args = build_run_args(&new_name, network, cfg);
        let arg_refs: Vec<&str> = run_args.iter().map(String::as_str).collect();
        info!(container = %new_name, image = %cfg.image, "Starting container");
        run_docker(&arg_refs).await?;

        if let Err(err) = wait_ready(&new_name, READINESS_TIMEOUT).await {
            warn!(container = %new_name, "Container failed readiness check, cleaning up");
            let _ = remove_container(&new_name).await;
            return Err(err);
        }

        let ip = inspect_ip(&new_name, cfg.network_mode.as_deref(), network).await?;
        info!(container = %new_name, ?ip, "Container ready");

        applied.insert(
            name.clone(),
            AppliedContainer {
                active_slot: new_slot,
                ip,
            },
        );

        if !cfg.stop_first {
            if let Some(prev) = old_entry {
                cleanup.push(container_name(service, name, prev.active_slot));
            }
        }
    }

    Ok(PreparedContainers { applied, cleanup })
}

pub async fn cleanup_containers(names: &[String]) -> Result<()> {
    for name in names {
        info!(container = %name, "Removing old container");
        remove_container(name).await?;
    }
    Ok(())
}

pub async fn remove_all_containers(
    service: &str,
    old: &BTreeMap<String, AppliedContainer>,
) -> Result<()> {
    let names: Vec<String> = old
        .iter()
        .map(|(name, state)| container_name(service, name, state.active_slot))
        .collect();
    cleanup_containers(&names).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_container_names() {
        assert_eq!(container_name("myapp", "web", Some(Slot::A)), "myapp-web-a");
        assert_eq!(container_name("myapp", "db", Some(Slot::B)), "myapp-db-b");
        assert_eq!(container_name("flag10", "bot", None), "flag10-bot");
    }

    #[test]
    fn builds_docker_run_args() {
        let mut env = BTreeMap::new();
        env.insert("RUST_LOG".to_owned(), "info".to_owned());

        let mut labels = BTreeMap::new();
        labels.insert("app".to_owned(), "web".to_owned());

        let cfg = ContainerConfig {
            image: "nginx:alpine".to_owned(),
            environment: env,
            volumes: vec!["/host:/cont".to_owned()],
            ports: vec!["8080:80".to_owned()],
            command: vec!["-g".to_owned(), "daemon off;".to_owned()],
            entrypoint: vec!["/bin/sh".to_owned(), "-c".to_owned()],
            restart: "unless-stopped".to_owned(),
            labels,
            user: Some("1000:1000".to_owned()),
            working_dir: Some("/app".to_owned()),
            network_mode: None,
            read_only: true,
            tty: true,
            stop_signal: Some("SIGKILL".to_owned()),
            stop_first: false,
        };

        let args = build_run_args("myapp-web-a", "songren", &cfg);
        assert_eq!(
            args,
            vec![
                "run",
                "-d",
                "--name",
                "myapp-web-a",
                "--restart",
                "unless-stopped",
                "--network",
                "songren",
                "--user",
                "1000:1000",
                "--workdir",
                "/app",
                "--read-only",
                "-t",
                "--stop-signal",
                "SIGKILL",
                "-e",
                "RUST_LOG=info",
                "-v",
                "/host:/cont",
                "-p",
                "8080:80",
                "--label",
                "app=web",
                "--entrypoint",
                "/bin/sh",
                "nginx:alpine",
                "-c",
                "-g",
                "daemon off;",
            ]
        );
    }
}
