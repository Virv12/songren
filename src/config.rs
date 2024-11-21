use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Var {
    Port,
    ImageId { image: String },
}

#[derive(Debug, Deserialize)]
pub(crate) struct DockerModule {
    pub(crate) content: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct NginxModule {
    pub(crate) content: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    #[serde(default)]
    pub(crate) vars: BTreeMap<String, Var>,
    pub(crate) docker: Option<DockerModule>,
    pub(crate) nginx: Option<NginxModule>,
}
