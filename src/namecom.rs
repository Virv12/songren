use std::sync::LazyLock;

use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};

const USERNAME: LazyLock<String> = LazyLock::new(|| {
    std::env::var("NAMECOM_USERNAME").expect("NAMECOM_USERNAME environment variable is not set")
});
const TOKEN: LazyLock<String> = LazyLock::new(|| {
    std::env::var("NAMECOM_TOKEN").expect("NAMECOM_TOKEN environment variable is not set")
});

static CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(rename_all = "camelCase")]
struct Record {
    ttl: i32,
    r#type: String,
    answer: String,
    domain_name: String,
    fqdn: String,
    host: String,
    id: i32,
    // priority: i32,
}

async fn list_record(domain: &str) -> Result<Vec<Record>> {
    #[derive(Debug, Deserialize)]
    struct Out {
        // total_count: i32,
        // from: i32,
        // to: i32,
        records: Vec<Record>,
        // last_page: i32,
        // next_page: i32,
    }

    let req = CLIENT
        .get(format!(
            "https://api.name.com/core/v1/domains/{domain}/records"
        ))
        .basic_auth(&*USERNAME, Some(&*TOKEN));
    let res = req.send().await?;
    res.error_for_status_ref()?;
    let out: Out = res.json().await?;

    Ok(out.records)
}

async fn delete_record(domain: &str, id: i32) -> Result<()> {
    let req = CLIENT
        .delete(format!(
            "https://api.name.com/core/v1/domains/{domain}/records/{id}"
        ))
        .basic_auth(&*USERNAME, Some(&*TOKEN));
    let res = req.send().await?;
    res.error_for_status_ref()?;
    Ok(())
}

async fn create_record(domain: &str, r#type: Type, host: &str, answer: &str) -> Result<()> {
    #[derive(Debug, Serialize)]
    struct Body {
        answer: String,
        host: String,
        r#type: String,
    }

    let req = CLIENT
        .post(format!(
            "https://api.name.com/core/v1/domains/{domain}/records"
        ))
        .basic_auth(&*USERNAME, Some(&*TOKEN))
        .json(&Body {
            r#type: r#type.as_str().to_owned(),
            host: host.to_owned(),
            answer: answer.to_owned(),
        });
    let res = req.send().await?;
    res.error_for_status_ref()?;
    Ok(())
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone, Copy)]
#[allow(clippy::upper_case_acronyms)]
enum Type {
    A,
    AAAA,
    CNAME,
}

impl Type {
    fn as_str(&self) -> &'static str {
        match self {
            Type::A => "A",
            Type::AAAA => "AAAA",
            Type::CNAME => "CNAME",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub struct NamecomModule {
    r#type: Type,
    domain: String,
    host: String,
    answer: String,
}

pub async fn namecom_update(
    _service: &str,
    old: &Option<NamecomModule>,
    new: &Option<NamecomModule>,
) -> Result<()> {
    if old == new {
        return Ok(());
    }

    if let Some(NamecomModule {
        r#type,
        domain,
        host,
        answer,
    }) = old
    {
        let records = list_record(domain).await?;
        let id = records
            .into_iter()
            .find(|rec| rec.r#type == r#type.as_str() && &rec.host == host && &rec.answer == answer)
            .map(|rec| rec.id);
        if let Some(id) = id {
            delete_record(domain, id).await?;
        }
    }

    if let Some(NamecomModule {
        r#type,
        domain,
        host,
        answer,
    }) = new
    {
        create_record(domain, *r#type, host, answer).await?;
    }

    Ok(())
}
