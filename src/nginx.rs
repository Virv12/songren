use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{bail, Context, Result};
use tokio::process::Command;
use tracing::info;

use crate::config::{AppliedContainer, GlobalConfig, NginxConfig};

pub fn render_nginx_config(
    template: &str,
    containers: &BTreeMap<String, AppliedContainer>,
    sites: &BTreeMap<String, PathBuf>,
) -> Result<String> {
    let mut rendered = template.to_owned();

    for (name, container) in containers {
        let placeholder = format!("{{docker.{name}.ip}}");
        if rendered.contains(&placeholder) {
            let ip = container.ip.with_context(|| {
                format!("Nginx config references {placeholder}, but container '{name}' has no IP")
            })?;
            rendered = rendered.replace(&placeholder, &ip.to_string());
        }
    }

    for (name, path) in sites {
        let placeholder = format!("{{git.{name}.site}}");
        if rendered.contains(&placeholder) {
            rendered = rendered.replace(&placeholder, &path.display().to_string());
        }
    }

    Ok(rendered)
}

async fn validate_nginx() -> Result<()> {
    let output = Command::new("nginx")
        .arg("-t")
        .output()
        .await
        .context("Failed to execute nginx -t")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("nginx -t failed: {}", stderr.trim());
    }
    Ok(())
}

async fn reload_nginx() -> Result<()> {
    let output = Command::new("systemctl")
        .args(["reload", "nginx"])
        .output()
        .await
        .context("Failed to execute systemctl reload nginx")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("systemctl reload nginx failed: {}", stderr.trim());
    }
    Ok(())
}

pub async fn nginx_update(
    cfg: &GlobalConfig,
    service: &str,
    module: Option<&NginxConfig>,
    containers: &BTreeMap<String, AppliedContainer>,
    sites: &BTreeMap<String, PathBuf>,
) -> Result<()> {
    let path = cfg.nginx_dir.join(format!("{service}.conf"));
    let prev = std::fs::read_to_string(&path).ok();

    let changed = match module {
        Some(NginxConfig { content }) => {
            let rendered = render_nginx_config(content, containers, sites)?;
            if prev.as_deref() == Some(rendered.as_str()) {
                false
            } else {
                std::fs::write(&path, &rendered)
                    .with_context(|| format!("Failed to write nginx config: {}", path.display()))?;
                true
            }
        }
        None => {
            if prev.is_some() {
                std::fs::remove_file(&path).with_context(|| {
                    format!("Failed to remove nginx config: {}", path.display())
                })?;
                true
            } else {
                false
            }
        }
    };

    if !changed {
        return Ok(());
    }

    if let Err(err) = validate_nginx().await {
        // Restore previous state so Nginx is never left with a broken config.
        match prev {
            Some(old) => {
                let _ = std::fs::write(&path, old);
            }
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
        return Err(err);
    }

    info!(service, "Reloading nginx");
    reload_nginx().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Slot;

    #[test]
    fn renders_container_ips_and_site_paths() {
        let mut containers = BTreeMap::new();
        containers.insert(
            "web".to_owned(),
            AppliedContainer {
                active_slot: Some(Slot::A),
                ip: Some("172.18.0.10".parse().unwrap()),
            },
        );
        let mut sites = BTreeMap::new();
        sites.insert("docs".to_owned(), PathBuf::from("/var/lib/songren/sites/app/docs"));

        let template = "proxy_pass http://{docker.web.ip}:8080;\nroot {git.docs.site};";
        let rendered = render_nginx_config(template, &containers, &sites).unwrap();
        assert_eq!(
            rendered,
            "proxy_pass http://172.18.0.10:8080;\nroot /var/lib/songren/sites/app/docs;"
        );
    }

    #[test]
    fn errors_when_referenced_container_has_no_ip() {
        let mut containers = BTreeMap::new();
        containers.insert(
            "web".to_owned(),
            AppliedContainer {
                active_slot: None,
                ip: None,
            },
        );

        let err = render_nginx_config(
            "proxy_pass http://{docker.web.ip}:8080;",
            &containers,
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("{docker.web.ip}"));
    }
}
