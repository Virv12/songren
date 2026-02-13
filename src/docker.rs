use std::path::PathBuf;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub struct DockerModule {
    pub content: String,
}

pub async fn docker_pre(service: &str, module: &Option<DockerModule>) -> Result<()> {
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

pub async fn docker_post(service: &str, module: &Option<DockerModule>) -> Result<()> {
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
