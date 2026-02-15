use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use axum::{extract, http::StatusCode, routing::post, Router};
use futures::{StreamExt, TryFutureExt};
use rand::prelude::Distribution;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::{
    dnsmasq::{dnsmasq_update, DnsmasqModule},
    docker::{docker_post, docker_pre, DockerModule},
    namecom::{namecom_update, NamecomModule},
    nginx::{nginx_update, NginxModule},
};

mod dnsmasq;
mod docker;
mod namecom;
mod nginx;

const LISTEN_ON: &str = match option_env!("LISTEN_ON") {
    Some(s) => s,
    None => "127.0.0.1:8000",
};

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
#[serde(tag = "type")]
#[serde(rename_all = "kebab-case")]
enum Var {
    Port,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Default, Clone)]
struct Config {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    vars: BTreeMap<String, Var>,
    docker: Option<DockerModule>,
    nginx: Option<NginxModule>,
    dnsmasq: Option<DnsmasqModule>,
    namecom: Option<NamecomModule>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
struct ConfigWithVars {
    #[serde(flatten)]
    inner: Config,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    vars_values: BTreeMap<String, String>,
}

async fn evaluate_var(var: &Var) -> Result<String> {
    match var {
        // TODO: stable port numbers
        Var::Port => Ok(rand::distributions::Uniform::from(12000..13000)
            .sample(&mut rand::thread_rng())
            .to_string()),
    }
}

fn apply(s: String, vars: &BTreeMap<String, String>) -> String {
    vars.iter().fold(s, |s, (name, value)| {
        s.replace(&format!("{{{name}}}"), value)
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let app = Router::new()
        .route(
            "/update/:service",
            post(|extract::Path(service): extract::Path<String>| {
                update(service)
                    .map_ok(|_| StatusCode::NO_CONTENT)
                    .map_err(|err| {
                        eprintln!("Error: {:?}", err);
                        StatusCode::INTERNAL_SERVER_ERROR
                    })
            }),
        )
        .route(
            "/update-config",
            post(|| {
                update_config()
                    .map_ok(|_| StatusCode::NO_CONTENT)
                    .map_err(|err| {
                        eprintln!("Error: {:?}", err);
                        StatusCode::INTERNAL_SERVER_ERROR
                    })
            }),
        );

    let listener = TcpListener::bind(LISTEN_ON).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn try_read(path: &Path) -> Result<Option<String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("Failed to read file: {:?}", path)),
    }
}

async fn update(service: String) -> Result<()> {
    println!("Updating: {service}");

    let config_path = PathBuf::from(format!("config/{service}.toml"));
    let config_str = try_read(&config_path).await?;
    let mut config: ConfigWithVars = config_str
        .as_deref()
        .map(toml::from_str)
        .transpose()
        .context("Failed to parse config file")?
        .unwrap_or_default();

    let infrastructure_path = PathBuf::from(format!("infrastructure/{service}.toml"));
    let infrastructure_str = try_read(&infrastructure_path).await?;
    let mut infrastructure: Config = infrastructure_str
        .as_deref()
        .map(toml::from_str)
        .transpose()
        .context("Failed to parse infrastructure file")?
        .unwrap_or_default();

    if config_str.is_none() && infrastructure_str.is_none() {
        bail!("No config or infrastructure file found");
    }

    let vars: BTreeMap<_, _> = futures::stream::iter(&infrastructure.vars)
        .then(|(name, var)| async move {
            let value = evaluate_var(var).await.unwrap();
            (name.clone(), value)
        })
        .collect()
        .await;

    let infrastructure_with_vars = ConfigWithVars {
        inner: infrastructure.clone(),
        vars_values: vars.clone(),
    };

    if let Some(DockerModule { content }) = &mut config.inner.docker {
        *content = apply(std::mem::take(content), &config.vars_values);
    }
    if let Some(NginxModule { content }) = &mut config.inner.nginx {
        *content = apply(std::mem::take(content), &config.vars_values);
    }

    if let Some(DockerModule { content }) = &mut infrastructure.docker {
        *content = apply(std::mem::take(content), &vars);
    }
    if let Some(NginxModule { content }) = &mut infrastructure.nginx {
        *content = apply(std::mem::take(content), &vars);
    }

    docker_pre(&service, &infrastructure.docker)
        .await
        .context("Failed to start container")?;

    nginx_update(&service, &infrastructure.nginx)
        .await
        .context("Failed to update nginx config")?;

    docker_post(&service, &infrastructure.docker)
        .await
        .context("Failed to clean up container")?;

    dnsmasq_update(&service, &infrastructure.dnsmasq)
        .await
        .context("Failed to update dnsmasq config")?;

    namecom_update(&service, &config.inner.namecom, &infrastructure.namecom)
        .await
        .context("Failed to update name.com config")?;

    if infrastructure_str.is_some() {
        tokio::fs::write(&config_path, toml::to_string(&infrastructure_with_vars)?)
            .await
            .context("Failed to write config file")?;
    } else {
        tokio::fs::remove_file(&config_path).await?;
    }

    Ok(())
}

async fn update_config() -> Result<()> {
    println!("Updating config");

    let mut services = Vec::new();

    let mut iter = tokio::fs::read_dir("config").await?;
    while let Some(entry) = iter.next_entry().await? {
        let path = entry.path();
        let service = path.file_stem().unwrap().to_str().unwrap();
        if path.extension() != Some("toml".as_ref()) {
            continue;
        }
        services.push(service.to_owned());
    }

    let mut iter = tokio::fs::read_dir("infrastructure").await?;
    while let Some(entry) = iter.next_entry().await? {
        let path = entry.path();
        let service = path.file_stem().unwrap().to_str().unwrap();
        if path.extension() != Some("toml".as_ref()) {
            continue;
        }
        services.push(service.to_owned());
    }

    services.sort();
    services.dedup();

    for service in services {
        // TODO: doing the same work twice
        let config_path = PathBuf::from(format!("config/{service}.toml"));
        let config_str = try_read(&config_path).await?;
        let config: Option<Config> = config_str
            .as_deref()
            .map(toml::from_str)
            .transpose()
            .context("Failed to parse config file")?;

        let infrastrucure_path = PathBuf::from(format!("infrastructure/{service}.toml"));
        let infrastructure_str = try_read(&infrastrucure_path).await?;
        let infrastructure: Option<Config> = infrastructure_str
            .as_deref()
            .map(toml::from_str)
            .transpose()
            .context("Failed to parse infrastructure file")?;

        if config != infrastructure {
            update(service.to_owned()).await?;
        }
    }

    Ok(())
}
