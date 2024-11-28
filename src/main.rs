use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{bail, ensure, Context, Result};
use axum::{extract, http::StatusCode, routing::post, Router};
use config::{Config, ConfigWithVars, DockerModule, GitModule, NginxModule, Var};
use futures::{StreamExt, TryFutureExt};
use rand::prelude::Distribution;

mod config;

const LISTEN_ON: &str = match option_env!("LISTEN_ON") {
    Some(s) => s,
    None => "127.0.0.1:8000",
};

async fn docker_image_id(image: &str) -> Result<String> {
    // TODO: consider using the bollard crate

    let status = tokio::process::Command::new("docker")
        .arg("pull")
        .arg(image)
        .status()
        .await?;
    ensure!(status.success(), "Failed to pull image");

    let output = tokio::process::Command::new("docker")
        .arg("image")
        .arg("inspect")
        .arg("--format")
        .arg("{{.Id}}")
        .arg(image)
        .stdout(Stdio::piped())
        .output()
        .await?;
    ensure!(output.status.success(), "Failed to get image ID");
    ensure!(output.stdout.starts_with(b"sha256:"), "Invalid image ID");
    Ok(std::str::from_utf8(&output.stdout[7..19])?
        .trim()
        .to_string())
}

async fn evaluate_var(var: &Var) -> Result<String> {
    match var {
        // TODO: stable port numbers
        Var::Port => Ok(rand::distributions::Uniform::from(12000..13000)
            .sample(&mut rand::thread_rng())
            .to_string()),
        Var::ImageId { image } => docker_image_id(image).await,
    }
}

fn apply(s: String, vars: &BTreeMap<String, String>) -> String {
    vars.iter().fold(s, |s, (name, value)| {
        s.replace(&format!("{{{name}}}"), value)
    })
}

#[tokio::main]
async fn main() {
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
    // .route("/update-all", )

    let listener = tokio::net::TcpListener::bind(LISTEN_ON).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

//async fn update_all() -> Result<()> {
//    let mut iter = tokio::fs::read_dir("infrastructure")
//        .await
//        .context("Failed to read containers directory")?;
//    while let Some(compose) = iter
//        .next_entry()
//        .await
//        .context("Failed to read next entry")?
//    {
//        let path = compose.path();
//        update(&path)
//            .await
//            .with_context(|| format!("Failed to update container: {:?}", path))?;
//    }
//    Ok(())
//}

async fn nginx_update(service: &str, module: &Option<NginxModule>) -> Result<()> {
    let mut update = false;

    let path = PathBuf::from(format!("nginx/{service}.conf"));

    if let Some(NginxModule { content }) = module {
        tokio::fs::write(&path, content)
            .await
            .context("Failed to write nginx config file")?;
        update = true;
    } else if path.exists() {
        tokio::fs::remove_file(&path).await?;
        update = true;
    }

    if update {
        let status = tokio::process::Command::new("systemctl")
            .arg("reload")
            .arg("nginx")
            .status()
            .await?;
        ensure!(status.success(), "Failed to reload nginx");
    }

    Ok(())
}

async fn docker_pre(service: &str, module: &Option<DockerModule>) -> Result<()> {
    if let Some(DockerModule { content }) = module {
        let path = PathBuf::from(format!("docker/{service}.yml"));

        tokio::fs::write(&path, content)
            .await
            .context("Failed to write docker-compose file")?;

        let status = tokio::process::Command::new("docker")
            .arg("compose")
            .arg("--file")
            .arg(path)
            .arg("--project-name")
            .arg(service)
            .arg("up")
            .arg("--detach")
            .arg("--pull")
            .arg("always")
            .status()
            .await?;
        ensure!(status.success(), "Failed to update container");
    }
    Ok(())
}

async fn docker_post(service: &str, module: &Option<DockerModule>) -> Result<()> {
    let path = PathBuf::from(format!("docker/{service}.yml"));
    if module.is_some() {
        let status = tokio::process::Command::new("docker")
            .arg("compose")
            .arg("--file")
            .arg(path)
            .arg("--project-name")
            .arg(service)
            .arg("up")
            .arg("--detach")
            .arg("--remove-orphans")
            .status()
            .await?;
        ensure!(status.success(), "Failed to update container");
    } else if path.exists() {
        let status = tokio::process::Command::new("docker")
            .arg("compose")
            .arg("--project-name")
            .arg(service)
            .arg("down")
            .arg("--remove-orphans")
            .status()
            .await?;
        ensure!(status.success(), "Failed to update container");
        tokio::fs::remove_file(&path).await?;
    }
    Ok(())
}

async fn git_update(
    _service: &str,
    old: &Option<GitModule>,
    new: &Option<GitModule>,
) -> Result<()> {
    if let Some(GitModule { repo, dir }) = new {
        let dir = Path::new(dir);
        if !dir.exists() {
            tokio::fs::create_dir_all(&dir).await?;
            let status = tokio::process::Command::new("git")
                .arg("clone")
                .arg(repo)
                .arg(dir)
                .status()
                .await?;
            ensure!(status.success(), "Failed to clone git repository");
        } else {
            let status = tokio::process::Command::new("git")
                .current_dir(dir)
                .arg("remote")
                .arg("set-url")
                .arg("origin")
                .arg(repo)
                .status()
                .await?;
            ensure!(status.success(), "Failed to set git repository URL");
            let status = tokio::process::Command::new("git")
                .current_dir(dir)
                .arg("pull")
                .status()
                .await?;
            ensure!(status.success(), "Failed to pull git repository");
        }
    }

    if let Some(GitModule { dir, .. }) = old {
        let dir = Path::new(dir);
        if dir.exists() && Some(dir) != new.as_ref().map(|m| Path::new(&m.dir)) {
            tokio::fs::remove_dir_all(&dir).await?;
        }
    }

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

    git_update(&service, &config.inner.git, &infrastructure.git)
        .await
        .context("Failed to update git repository")?;

    nginx_update(&service, &infrastructure.nginx)
        .await
        .context("Failed to update nginx config")?;

    docker_post(&service, &infrastructure.docker)
        .await
        .context("Failed to clean up container")?;

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

    let config_iter = tokio_stream::wrappers::ReadDirStream::new(
        tokio::fs::read_dir("config")
            .await
            .context("Failed to read config directory")?,
    );
    let infrastructure_iter = tokio_stream::wrappers::ReadDirStream::new(
        tokio::fs::read_dir("infrastructure")
            .await
            .context("Failed to read config directory")?,
    );
    // TODO: deduplicate elements
    let mut iter = config_iter.chain(infrastructure_iter);

    while let Some(entry) = iter.next().await.transpose()? {
        let path = entry.path();
        let service = path.file_stem().unwrap().to_str().unwrap();

        if path.extension() != Some("toml".as_ref()) {
            continue;
        }

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
