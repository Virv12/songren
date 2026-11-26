use std::{collections::BTreeMap, fmt, net::IpAddr, path::PathBuf};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Validate that a service name is safe for filesystem paths, Docker container
/// names, and Nginx/dnsmasq config filenames.
pub fn validate_service_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("Service name cannot be empty");
    };
    if name.len() > 63 {
        bail!("Service name must be at most 63 characters");
    }
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        bail!("Service name must start with a lowercase ASCII letter or digit: {name}");
    }
    if !chars
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.')
    {
        bail!(
            "Service name '{name}' contains invalid characters (allowed: lowercase a-z, 0-9, '-', '_', '.')"
        );
    }

    Ok(())
}

/// DNS record type (e.g. "A", "AAAA", "CNAME", "TXT", "MX", "SRV", etc.).
/// Stored as an uppercase string so any DNS record type is supported.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DnsRecordType(String);

impl DnsRecordType {
    pub fn from_string(mut raw: String) -> Result<Self> {
        if raw.is_empty() || !raw.chars().all(|c| c.is_ascii_alphanumeric()) {
            bail!("Invalid DNS record type: '{raw}'");
        }
        raw.make_ascii_uppercase();
        Ok(Self(raw))
    }

    pub fn parse(raw: &str) -> Result<Self> {
        Self::from_string(raw.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for DnsRecordType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DnsRecordType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_string(s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub struct ContainerConfig {
    pub image: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entrypoint: Vec<String>,
    #[serde(default = "default_restart")]
    pub restart: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub user: Option<String>,
    pub working_dir: Option<String>,
    pub network_mode: Option<String>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub tty: bool,
    pub stop_signal: Option<String>,
    /// Songren update strategy: when true, stop the existing container before
    /// starting the new one (for stateful/single-instance services).
    #[serde(default)]
    pub stop_first: bool,
}

fn default_restart() -> String {
    "unless-stopped".to_owned()
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub struct GitConfig {
    pub repo: String,
    pub branch: Option<String>,
    /// Optional custom clone destination. Defaults to `<sites_dir>/<service>/<name>`.
    pub path: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub struct NginxConfig {
    /// Raw Nginx configuration. Supports `{ip.<name>}` and `{site.<name>}` placeholders.
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub struct DnsmasqConfig {
    #[serde(rename = "type")]
    pub record_type: DnsRecordType,
    pub host: String,
    pub target: String,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub struct NamecomConfig {
    #[serde(rename = "type")]
    pub record_type: DnsRecordType,
    pub domain: String,
    pub host: String,
    pub answer: String,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Default, Clone)]
pub struct ServiceConfig {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub containers: BTreeMap<String, ContainerConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub git: BTreeMap<String, GitConfig>,
    pub nginx: Option<NginxConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dnsmasq: Vec<DnsmasqConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namecom: Vec<NamecomConfig>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub fn next(self) -> Self {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Slot::A => "a",
            Slot::B => "b",
        })
    }
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub struct NamecomCredentials {
    pub username: String,
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub struct GlobalConfig {
    pub listen_on: String,
    #[serde(default = "default_docker_network")]
    pub docker_network: String,
    pub auth_token: Option<String>,
    pub namecom: Option<NamecomCredentials>,
    #[serde(default)]
    pub services_dir: PathBuf,
    #[serde(default)]
    pub state_dir: PathBuf,
    #[serde(default)]
    pub sites_dir: PathBuf,
    #[serde(default)]
    pub nginx_dir: PathBuf,
    #[serde(default)]
    pub dnsmasq_dir: PathBuf,
}

impl GlobalConfig {
    pub fn resolve_dirs(&mut self, base: &std::path::Path) {
        if self.services_dir.as_os_str().is_empty() {
            self.services_dir = base.join("services");
        }
        if self.state_dir.as_os_str().is_empty() {
            self.state_dir = base.join("state");
        }
        if self.sites_dir.as_os_str().is_empty() {
            self.sites_dir = base.join("sites");
        }
        if self.nginx_dir.as_os_str().is_empty() {
            self.nginx_dir = base.join("nginx");
        }
        if self.dnsmasq_dir.as_os_str().is_empty() {
            self.dnsmasq_dir = base.join("dnsmasq");
        }
    }

    pub fn desired_path(&self, service: &str) -> PathBuf {
        self.services_dir.join(format!("{service}.toml"))
    }

    pub fn applied_path(&self, service: &str) -> PathBuf {
        self.state_dir.join(format!("{service}.toml"))
    }
}

fn default_docker_network() -> String {
    "songren".to_owned()
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone, Default)]
pub struct AppliedContainer {
    pub active_slot: Option<Slot>,
    pub ip: Option<IpAddr>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone, Default)]
pub struct AppliedState {
    pub config: ServiceConfig,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub containers: BTreeMap<String, AppliedContainer>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_service_names() {
        for valid in [
            "app",
            "my-app",
            "app_1",
            "0service",
            "my.app",
            "app.example.com",
        ] {
            assert!(
                validate_service_name(valid).is_ok(),
                "{valid} should be valid"
            );
        }

        for invalid in [
            "",
            "../etc",
            "app/sub",
            "MyApp",
            "-app",
            "_app",
            ".app",
            &"a".repeat(64),
        ] {
            assert!(
                validate_service_name(invalid).is_err(),
                "{invalid} should be invalid"
            );
        }
    }

    #[test]
    fn parses_service_config_toml() {
        let raw = r#"
[containers.web]
image = "ghcr.io/virv12/myapp:latest"
volumes = ["/data/myapp:/app/data"]
read_only = true
tty = true
stop_signal = "SIGKILL"

[containers.web.environment]
RUST_LOG = "info"

[containers.db]
image = "postgres:16"
stop_first = true

[nginx]
content = """
server {
    listen 80;
    server_name app.example.com;
    location / {
        proxy_pass http://{ip.web}:8080;
    }
}
"""

[[dnsmasq]]
type = "A"
host = "app.internal"
target = "192.168.1.10"

[[dnsmasq]]
type = "AAAA"
host = "app.internal"
target = "::1"

[[namecom]]
type = "A"
domain = "example.com"
host = "app"
answer = "203.0.113.10"

[git.site]
repo = "https://github.com/virv12/site.git"
branch = "main"
path = "/var/www/site"
"#;

        let cfg: ServiceConfig = toml::from_str(raw).unwrap();
        let web = cfg.containers.get("web").unwrap();
        assert_eq!(web.image, "ghcr.io/virv12/myapp:latest");
        assert_eq!(web.volumes, vec!["/data/myapp:/app/data"]);
        assert_eq!(web.restart, "unless-stopped");
        assert!(web.read_only);
        assert!(web.tty);
        assert_eq!(web.stop_signal.as_deref(), Some("SIGKILL"));
        assert!(!web.stop_first);
        assert_eq!(
            web.environment.get("RUST_LOG").map(String::as_str),
            Some("info")
        );

        let db = cfg.containers.get("db").unwrap();
        assert!(db.stop_first);

        assert!(cfg
            .nginx
            .as_ref()
            .unwrap()
            .content
            .contains("proxy_pass http://{ip.web}:8080;"));
        assert_eq!(cfg.dnsmasq.len(), 2);
        assert_eq!(
            cfg.dnsmasq[0].record_type,
            DnsRecordType::parse("A").unwrap()
        );
        assert_eq!(cfg.namecom.len(), 1);
        assert_eq!(cfg.namecom[0].host, "app");
        let git = cfg.git.get("site").unwrap();
        assert_eq!(git.repo, "https://github.com/virv12/site.git");
        assert_eq!(git.branch.as_deref(), Some("main"));
        assert_eq!(
            git.path.as_deref(),
            Some(std::path::Path::new("/var/www/site"))
        );
    }

    #[test]
    fn parses_arbitrary_dns_record_types() {
        #[derive(Deserialize)]
        struct Wrapper {
            t: DnsRecordType,
        }

        let txt: Wrapper = toml::from_str(r#"t = "txt""#).unwrap();
        assert_eq!(txt.t.as_str(), "TXT");

        let caa: Wrapper = toml::from_str(r#"t = "CAA""#).unwrap();
        assert_eq!(caa.t.as_str(), "CAA");

        assert!(toml::from_str::<Wrapper>(r#"t = """#).is_err());
        assert!(toml::from_str::<Wrapper>(r#"t = "bad type!""#).is_err());
    }

    #[test]
    fn slot_cycles_between_a_and_b() {
        assert_eq!(Slot::A.next(), Slot::B);
        assert_eq!(Slot::B.next(), Slot::A);
    }

    #[test]
    fn parses_global_config_with_defaults() {
        let cfg: GlobalConfig = toml::from_str(r#"listen_on = "127.0.0.1:8000""#).unwrap();
        assert_eq!(cfg.listen_on, "127.0.0.1:8000");
        assert_eq!(cfg.docker_network, "songren");
        assert!(cfg.auth_token.is_none());
        assert!(cfg.namecom.is_none());

        assert!(toml::from_str::<GlobalConfig>("").is_err());

        let custom: GlobalConfig = toml::from_str(
            r#"
listen_on = "0.0.0.0:9000"
auth_token = "secret"

[namecom]
username = "user"
token = "tok"
"#,
        )
        .unwrap();
        assert_eq!(custom.listen_on, "0.0.0.0:9000");
        assert_eq!(custom.auth_token.as_deref(), Some("secret"));
        assert_eq!(custom.namecom.as_ref().unwrap().username, "user");
    }
}
