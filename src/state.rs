use std::path::Path;

use anyhow::{Context, Result};

use crate::config::{validate_service_name, AppliedState, GlobalConfig, ServiceConfig};

pub fn ensure_dirs(cfg: &GlobalConfig) -> Result<()> {
    for dir in [
        &cfg.services_dir,
        &cfg.state_dir,
        &cfg.sites_dir,
        &cfg.nginx_dir,
        &cfg.dnsmasq_dir,
    ] {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
pub enum ServiceDiff {
    Create(ServiceConfig),
    Update {
        old: AppliedState,
        new: ServiceConfig,
    },
    Delete(AppliedState),
    Unchanged,
}

pub fn compute_diff(
    desired: Option<ServiceConfig>,
    applied: Option<AppliedState>,
) -> ServiceDiff {
    match (desired, applied) {
        (Some(new), None) => ServiceDiff::Create(new),
        (Some(new), Some(old)) if old.config != new => ServiceDiff::Update { old, new },
        (Some(_), Some(_)) => ServiceDiff::Unchanged,
        (None, Some(old)) => ServiceDiff::Delete(old),
        (None, None) => ServiceDiff::Unchanged,
    }
}

fn try_read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("Failed to read file: {}", path.display())),
    }
}

pub fn load_global_config(base: impl AsRef<Path>) -> Result<GlobalConfig> {
    let base = base.as_ref();
    let path = base.join("songren.toml");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read global config: {}", path.display()))?;
    let mut cfg: GlobalConfig = toml::from_str(&raw)
        .with_context(|| format!("Failed to parse global config: {}", path.display()))?;
    cfg.resolve_dirs(base);
    Ok(cfg)
}

pub fn load_desired(cfg: &GlobalConfig, service: &str) -> Result<Option<ServiceConfig>> {
    validate_service_name(service)?;
    let path = cfg.desired_path(service);
    let Some(raw) = try_read(&path)? else {
        return Ok(None);
    };
    let parsed: ServiceConfig = toml::from_str(&raw)
        .with_context(|| format!("Failed to parse desired config: {}", path.display()))?;
    for name in parsed.containers.keys().chain(parsed.git.keys()) {
        validate_service_name(name)?;
    }
    Ok(Some(parsed))
}

pub fn load_applied(cfg: &GlobalConfig, service: &str) -> Result<Option<AppliedState>> {
    validate_service_name(service)?;
    let path = cfg.applied_path(service);
    let Some(raw) = try_read(&path)? else {
        return Ok(None);
    };
    let parsed: AppliedState = toml::from_str(&raw)
        .with_context(|| format!("Failed to parse applied state: {}", path.display()))?;
    Ok(Some(parsed))
}

pub fn save_applied(cfg: &GlobalConfig, service: &str, state: &AppliedState) -> Result<()> {
    validate_service_name(service)?;
    let path = cfg.applied_path(service);
    let serialized = toml::to_string_pretty(state)?;
    std::fs::write(&path, serialized)
        .with_context(|| format!("Failed to write applied state: {}", path.display()))
}

pub fn remove_applied(cfg: &GlobalConfig, service: &str) -> Result<()> {
    validate_service_name(service)?;
    let path = cfg.applied_path(service);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("Failed to remove state: {}", path.display())),
    }
}

fn collect_toml_stems(dir: &Path, out: &mut Vec<String>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(err).with_context(|| format!("Failed to read dir: {}", dir.display()))
        }
    };

    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            validate_service_name(stem)?;
            out.push(stem.to_owned());
        }
    }

    Ok(())
}

pub fn list_services(cfg: &GlobalConfig) -> Result<Vec<String>> {
    let mut services = Vec::new();
    collect_toml_stems(&cfg.services_dir, &mut services)?;
    collect_toml_stems(&cfg.state_dir, &mut services)?;
    services.sort();
    services.dedup();
    Ok(services)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::config::{AppliedContainer, ContainerConfig, Slot};

    fn sample_config(image: &str) -> ServiceConfig {
        let mut containers = std::collections::BTreeMap::new();
        containers.insert(
            "web".to_owned(),
            ContainerConfig {
                image: image.to_owned(),
                environment: Default::default(),
                volumes: Vec::new(),
                ports: Vec::new(),
                command: Vec::new(),
                entrypoint: Vec::new(),
                restart: "unless-stopped".to_owned(),
                labels: Default::default(),
                user: None,
                working_dir: None,
                network_mode: None,
                stop_first: false,
            },
        );
        ServiceConfig {
            containers,
            ..Default::default()
        }
    }

    #[test]
    fn computes_all_diff_variants() {
        let cfg_a = sample_config("app:1");
        let cfg_b = sample_config("app:2");
        let mut containers = std::collections::BTreeMap::new();
        containers.insert(
            "web".to_owned(),
            AppliedContainer {
                active_slot: Some(Slot::A),
                ip: Some("172.18.0.2".to_owned()),
            },
        );
        let applied_a = AppliedState {
            config: cfg_a.clone(),
            containers,
        };

        assert_eq!(
            compute_diff(Some(cfg_a.clone()), None),
            ServiceDiff::Create(cfg_a.clone())
        );
        assert_eq!(
            compute_diff(Some(cfg_a.clone()), Some(applied_a.clone())),
            ServiceDiff::Unchanged
        );
        assert_eq!(
            compute_diff(Some(cfg_b.clone()), Some(applied_a.clone())),
            ServiceDiff::Update {
                old: applied_a.clone(),
                new: cfg_b,
            }
        );
        assert_eq!(
            compute_diff(None, Some(applied_a.clone())),
            ServiceDiff::Delete(applied_a)
        );
        assert_eq!(compute_diff(None, None), ServiceDiff::Unchanged);
    }

    #[test]
    fn manages_dirs_state_and_service_listing() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("songren.toml"),
            r#"listen_on = "127.0.0.1:8000""#,
        )
        .unwrap();

        let global = load_global_config(tmp.path()).unwrap();
        ensure_dirs(&global).unwrap();

        let cfg = sample_config("app:latest");
        std::fs::write(
            global.desired_path("web"),
            toml::to_string_pretty(&cfg).unwrap(),
        )
        .unwrap();

        let mut containers = std::collections::BTreeMap::new();
        containers.insert(
            "web".to_owned(),
            AppliedContainer {
                active_slot: Some(Slot::A),
                ip: Some("172.18.0.5".to_owned()),
            },
        );
        let state = AppliedState {
            config: cfg.clone(),
            containers,
        };
        save_applied(&global, "web", &state).unwrap();
        save_applied(&global, "orphan", &state).unwrap();

        let loaded_desired = load_desired(&global, "web").unwrap().unwrap();
        assert_eq!(loaded_desired, cfg);
        let loaded_applied = load_applied(&global, "web").unwrap().unwrap();
        assert_eq!(loaded_applied, state);

        let services = list_services(&global).unwrap();
        assert_eq!(services, vec!["orphan", "web"]);

        remove_applied(&global, "orphan").unwrap();
        assert!(load_applied(&global, "orphan").unwrap().is_none());
    }
}
