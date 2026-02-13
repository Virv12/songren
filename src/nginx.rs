use std::path::PathBuf;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub struct NginxModule {
    pub content: String,
}

pub async fn nginx_update(service: &str, module: &Option<NginxModule>) -> Result<()> {
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
