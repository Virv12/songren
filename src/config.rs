use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
#[serde(tag = "type")]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Var {
    Port,
    ImageId { image: String },
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub(crate) struct DockerModule {
    pub(crate) content: String,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub(crate) struct NginxModule {
    pub(crate) content: String,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub(crate) struct GitModule {
    pub(crate) repo: String,
    pub(crate) dir: String,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Default, Clone)]
pub(crate) struct Config {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) vars: BTreeMap<String, Var>,
    pub(crate) docker: Option<DockerModule>,
    pub(crate) nginx: Option<NginxModule>,
    pub(crate) git: Option<GitModule>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub(crate) struct ConfigWithVars {
    #[serde(flatten)]
    pub(crate) inner: Config,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) vars_values: BTreeMap<String, String>,
}
