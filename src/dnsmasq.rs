use std::path::PathBuf;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
#[allow(clippy::upper_case_acronyms)]
enum Type {
    A,
    AAAA,
    CNAME,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub struct DnsmasqModule {
    r#type: Type,
    domain: String,
    target: String,
}

pub async fn dnsmasq_update(service: &str, module: &Option<DnsmasqModule>) -> Result<()> {
    let mut update = false;

    let path = PathBuf::from(format!("dnsmasq/{service}.conf"));

    if let Some(DnsmasqModule {
        r#type,
        domain,
        target,
    }) = module
    {
        let content = match r#type {
            Type::A => format!("host-record={domain},{target}"),
            Type::AAAA => format!("host-record={domain},{target}"),
            Type::CNAME => format!("cname={domain},{target}"),
        };
        tokio::fs::write(&path, content)
            .await
            .context("Failed to write dnsmasq config file")?;
        update = true;
    } else if path.exists() {
        tokio::fs::remove_file(&path).await?;
        update = true;
    }

    if update {
        let status = tokio::process::Command::new("systemctl")
            .arg("restart")
            .arg("dnsmasq")
            .status()
            .await?;
        ensure!(status.success(), "Failed to restart dnsmasq");
    }

    Ok(())
}
