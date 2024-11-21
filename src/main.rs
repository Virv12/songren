use std::{collections::HashMap, path::PathBuf, process::Stdio};

use anyhow::{ensure, Context, Result};
use axum::{extract, http::StatusCode, routing::post, Router};
use config::{Config, DockerModule, NginxModule, Var};
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

fn apply(s: String, vars: &HashMap<String, String>) -> String {
    vars.iter().fold(s, |s, (name, value)| {
        s.replace(&format!("{{{name}}}"), value)
    })
}

#[tokio::main]
async fn main() {
    let app = Router::new().route(
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
            // .route("/update-config", )
            // .route("/update-all", )
            ;

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

async fn update(service: String) -> Result<()> {
    println!("Updating: {service}");

    let config_path = PathBuf::from(format!("infrastructure/{service}.toml"));

    let config_str = tokio::fs::read_to_string(&config_path)
        .await
        .with_context(|| format!("Failed to read config file: {:?}", &config_path))?;
    let mut config: Config = toml::from_str(&config_str).context("Failed to parse config file")?;

    let vars: HashMap<_, _> = futures::stream::iter(config.vars)
        .then(|(name, var)| async move {
            let value = evaluate_var(&var).await.unwrap();
            (name, value)
        })
        .collect()
        .await;

    if let Some(DockerModule { content }) = &mut config.docker {
        *content = apply(std::mem::take(content), &vars);
    }
    if let Some(NginxModule { content }) = &mut config.nginx {
        *content = apply(std::mem::take(content), &vars);
    }

    docker_pre(&service, &config.docker)
        .await
        .context("Failed to start container")?;

    nginx_update(&service, &config.nginx)
        .await
        .context("Failed to update nginx config")?;

    docker_post(&service, &config.docker)
        .await
        .context("Failed to clean up container")?;

    Ok(())
}
