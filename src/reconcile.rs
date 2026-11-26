use std::collections::BTreeMap;

use anyhow::Result;
use tracing::{info, warn};

use crate::{
    config::{AppliedState, GlobalConfig},
    dnsmasq, docker, git, namecom, nginx,
    state::{self, ServiceDiff},
};

pub async fn reconcile_service(global: &GlobalConfig, service: &str, force: bool) -> Result<bool> {
    let desired = state::load_desired(global, service)?;
    let applied = state::load_applied(global, service)?;

    if desired.is_none() && applied.is_none() {
        return Ok(false);
    }

    if !force && state::compute_diff(desired.clone(), applied.clone()) == ServiceDiff::Unchanged {
        return Ok(true);
    }

    match desired {
        Some(new_cfg) => {
            info!(service, force, "Reconciling service");
            let old_containers = applied
                .as_ref()
                .map(|s| s.containers.clone())
                .unwrap_or_default();
            let old_git = applied
                .as_ref()
                .map(|s| s.config.git.clone())
                .unwrap_or_default();
            let old_namecom = applied
                .as_ref()
                .map(|s| s.namecom.as_slice())
                .unwrap_or(&[]);

            let prepared = docker::prepare_containers(
                service,
                &global.docker_network,
                &new_cfg.containers,
                &old_containers,
            )
            .await?;

            let worktrees = git::sync_git(global, service, &new_cfg.git, &old_git).await?;

            nginx::nginx_update(
                global,
                service,
                new_cfg.nginx.as_ref(),
                &prepared.applied,
                &worktrees,
            )
            .await?;

            docker::cleanup_containers(&prepared.cleanup).await?;
            dnsmasq::dnsmasq_update(global, service, &new_cfg.dnsmasq).await?;
            let applied_namecom =
                namecom::namecom_update(global.namecom.as_ref(), old_namecom, &new_cfg.namecom)
                    .await?;

            state::save_applied(
                global,
                service,
                &AppliedState {
                    config: new_cfg,
                    containers: prepared.applied,
                    namecom: applied_namecom,
                },
            )?;
        }
        None => {
            if let Some(old_state) = applied {
                info!(service, "Removing service");
                nginx::nginx_update(global, service, None, &BTreeMap::new(), &BTreeMap::new())
                    .await?;
                docker::remove_all_containers(service, &old_state.containers).await?;
                git::remove_all_git(global, service, &old_state.config.git)?;
                dnsmasq::dnsmasq_update(global, service, &[]).await?;
                namecom::namecom_update(global.namecom.as_ref(), &old_state.namecom, &[]).await?;
                state::remove_applied(global, service)?;
            }
        }
    }

    Ok(true)
}

pub async fn sync_all(global: &GlobalConfig) -> Result<()> {
    let services = state::list_services(global)?;
    info!(count = services.len(), "Syncing all services");
    for service in services {
        reconcile_service(global, &service, false).await?;
    }
    Ok(())
}

/// Re-inspect active container IPs on startup (e.g., after host reboot)
/// and update Nginx configs if any IP changed.
pub async fn refresh_ips(global: &GlobalConfig) -> Result<()> {
    let services = state::list_services(global)?;
    for service in services {
        let Some(mut applied) = state::load_applied(global, &service)? else {
            continue;
        };
        if applied.containers.is_empty() || applied.config.nginx.is_none() {
            continue;
        }

        let mut changed = false;
        for (name, c_state) in &mut applied.containers {
            let cname = docker::container_name(&service, name, c_state.active_slot);
            let network_mode = applied
                .config
                .containers
                .get(name)
                .and_then(|c| c.network_mode.as_deref());

            match docker::inspect_ip(&cname, network_mode, &global.docker_network).await {
                Ok(ip) if ip != c_state.ip => {
                    info!(service = %service, container = %cname, old = ?c_state.ip, new = ?ip, "Container IP changed");
                    c_state.ip = ip;
                    changed = true;
                }
                Ok(_) => {}
                Err(err) => {
                    warn!(service = %service, container = %cname, error = ?err, "Failed to inspect container IP on startup");
                }
            }
        }

        if changed {
            let worktrees: BTreeMap<_, _> = applied
                .config
                .git
                .iter()
                .map(|(name, g)| (name.clone(), git::worktree_path(global, &service, name, g)))
                .collect();

            nginx::nginx_update(
                global,
                &service,
                applied.config.nginx.as_ref(),
                &applied.containers,
                &worktrees,
            )
            .await?;
            state::save_applied(global, &service, &applied)?;
        }
    }
    Ok(())
}
