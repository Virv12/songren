use std::fmt::Write;

use anyhow::{bail, Context, Result};
use tokio::process::Command;
use tracing::info;

use crate::config::{DnsmasqConfig, GlobalConfig};

pub fn render_dnsmasq(entries: &[DnsmasqConfig]) -> Result<String> {
    let mut out = String::new();
    for entry in entries {
        match entry.record_type.as_str() {
            "A" | "AAAA" => writeln!(out, "host-record={},{}", entry.host, entry.target)?,
            "CNAME" => writeln!(out, "cname={},{}", entry.host, entry.target)?,
            "TXT" => writeln!(out, "txt-record={},\"{}\"", entry.host, entry.target)?,
            "PTR" => writeln!(out, "ptr-record={},{}", entry.host, entry.target)?,
            "CAA" => writeln!(out, "caa-record={},{}", entry.host, entry.target)?,
            other => bail!("Unsupported dnsmasq record type: {other}"),
        }
    }
    Ok(out)
}

pub async fn dnsmasq_update(
    cfg: &GlobalConfig,
    service: &str,
    entries: &[DnsmasqConfig],
) -> Result<()> {
    let path = cfg.dnsmasq_dir.join(format!("{service}.conf"));
    let prev = std::fs::read_to_string(&path).ok();

    let changed = if entries.is_empty() {
        if prev.is_some() {
            std::fs::remove_file(&path).with_context(|| {
                format!("Failed to remove dnsmasq config: {}", path.display())
            })?;
            true
        } else {
            false
        }
    } else {
        let rendered = render_dnsmasq(entries)?;
        if prev.as_deref() == Some(rendered.as_str()) {
            false
        } else {
            std::fs::write(&path, &rendered)
                .with_context(|| format!("Failed to write dnsmasq config: {}", path.display()))?;
            true
        }
    };

    if !changed {
        return Ok(());
    }

    info!(service, "Restarting dnsmasq");
    let output = Command::new("systemctl")
        .args(["restart", "dnsmasq"])
        .output()
        .await
        .context("Failed to execute systemctl restart dnsmasq")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("systemctl restart dnsmasq failed: {}", stderr.trim());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DnsRecordType;

    #[test]
    fn renders_multiple_dnsmasq_records() {
        let entries = vec![
            DnsmasqConfig {
                record_type: DnsRecordType::parse("A").unwrap(),
                host: "app.internal".to_owned(),
                target: "192.168.1.10".to_owned(),
            },
            DnsmasqConfig {
                record_type: DnsRecordType::parse("CNAME").unwrap(),
                host: "alias.internal".to_owned(),
                target: "app.internal".to_owned(),
            },
            DnsmasqConfig {
                record_type: DnsRecordType::parse("TXT").unwrap(),
                host: "app.internal".to_owned(),
                target: "v=1".to_owned(),
            },
        ];

        let rendered = render_dnsmasq(&entries).unwrap();
        assert_eq!(
            rendered,
            "host-record=app.internal,192.168.1.10\ncname=alias.internal,app.internal\ntxt-record=app.internal,\"v=1\"\n"
        );
    }

    #[test]
    fn rejects_unsupported_dnsmasq_record_type() {
        let entries = vec![DnsmasqConfig {
            record_type: DnsRecordType::parse("ANAME").unwrap(),
            host: "app.internal".to_owned(),
            target: "target.internal".to_owned(),
        }];
        assert!(render_dnsmasq(&entries).is_err());
    }
}
