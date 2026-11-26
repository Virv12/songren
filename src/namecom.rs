use std::sync::LazyLock;

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::{AppliedNamecomRecord, NamecomConfig, NamecomCredentials};

const DEFAULT_API_BASE: &str = "https://api.name.com";

static CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRecordResponse {
    id: i32,
}

async fn delete_record(
    api_base: &str,
    creds: &NamecomCredentials,
    domain: &str,
    id: i32,
) -> Result<()> {
    let res = CLIENT
        .delete(format!("{api_base}/core/v1/domains/{domain}/records/{id}"))
        .basic_auth(&creds.username, Some(&creds.token))
        .send()
        .await?;
    res.error_for_status_ref()?;
    Ok(())
}

async fn create_record(
    api_base: &str,
    creds: &NamecomCredentials,
    entry: &NamecomConfig,
) -> Result<i32> {
    #[derive(Serialize)]
    struct Body<'a> {
        r#type: &'a str,
        host: &'a str,
        answer: &'a str,
    }

    let res = CLIENT
        .post(format!(
            "{api_base}/core/v1/domains/{}/records",
            entry.domain
        ))
        .basic_auth(&creds.username, Some(&creds.token))
        .json(&Body {
            r#type: entry.record_type.as_str(),
            host: &entry.host,
            answer: &entry.answer,
        })
        .send()
        .await?;
    res.error_for_status_ref()?;
    let created: CreateRecordResponse = res.json().await?;
    Ok(created.id)
}

pub async fn namecom_update(
    creds: Option<&NamecomCredentials>,
    old: &[AppliedNamecomRecord],
    new: &[NamecomConfig],
) -> Result<Vec<AppliedNamecomRecord>> {
    namecom_update_with_base(DEFAULT_API_BASE, creds, old, new).await
}

async fn namecom_update_with_base(
    api_base: &str,
    creds: Option<&NamecomCredentials>,
    old: &[AppliedNamecomRecord],
    new: &[NamecomConfig],
) -> Result<Vec<AppliedNamecomRecord>> {
    let old_configs: Vec<&NamecomConfig> = old.iter().map(|r| &r.config).collect();
    if old_configs.len() == new.len() && old_configs.iter().zip(new.iter()).all(|(a, b)| *a == b) {
        return Ok(old.to_vec());
    }

    let creds = creds.context(
        "Name.com DNS records configured, but [namecom] credentials are missing in songren.toml",
    )?;

    for old_rec in old {
        if new.contains(&old_rec.config) {
            continue;
        }
        info!(domain = %old_rec.config.domain, host = %old_rec.config.host, id = old_rec.id, "Deleting Name.com DNS record");
        delete_record(api_base, creds, &old_rec.config.domain, old_rec.id).await?;
    }

    let mut applied = Vec::with_capacity(new.len());
    for entry in new {
        if let Some(existing) = old.iter().find(|r| &r.config == entry) {
            applied.push(existing.clone());
            continue;
        }

        info!(domain = %entry.domain, host = %entry.host, "Creating Name.com DNS record");
        let id = create_record(api_base, creds, entry).await?;
        applied.push(AppliedNamecomRecord {
            config: entry.clone(),
            id,
        });
    }

    Ok(applied)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        extract::{Path, State},
        routing::{delete, post},
        Json, Router,
    };
    use tokio::{net::TcpListener, sync::Mutex};

    use super::*;
    use crate::config::DnsRecordType;

    #[derive(Default)]
    struct MockState {
        deleted: Vec<i32>,
        created: Vec<(String, String, String)>,
    }

    #[tokio::test]
    async fn errors_gracefully_when_credentials_missing() {
        let new = vec![NamecomConfig {
            record_type: DnsRecordType::parse("A").unwrap(),
            domain: "example.com".to_owned(),
            host: "app".to_owned(),
            answer: "1.2.3.4".to_owned(),
        }];
        let err = namecom_update(None, &[], &new).await.unwrap_err();
        assert!(err.to_string().contains("credentials are missing"));
    }

    #[tokio::test]
    async fn deletes_by_stored_id_and_captures_created_id() {
        let state = Arc::new(Mutex::new(MockState::default()));

        let app = Router::new()
            .route(
                "/core/v1/domains/:domain/records",
                post(
                    |State(st): State<Arc<Mutex<MockState>>>,
                     Json(body): Json<serde_json::Value>| async move {
                        let mut st = st.lock().await;
                        st.created.push((
                            body["type"].as_str().unwrap().to_owned(),
                            body["host"].as_str().unwrap().to_owned(),
                            body["answer"].as_str().unwrap().to_owned(),
                        ));
                        Json(serde_json::json!({ "id": 99 }))
                    },
                ),
            )
            .route(
                "/core/v1/domains/:domain/records/:id",
                delete(
                    |State(st): State<Arc<Mutex<MockState>>>,
                     Path((_d, id)): Path<(String, i32)>| async move {
                        st.lock().await.deleted.push(id);
                        Json(serde_json::json!({}))
                    },
                ),
            )
            .with_state(state.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let creds = NamecomCredentials {
            username: "u".into(),
            token: "t".into(),
        };
        let old = vec![AppliedNamecomRecord {
            config: NamecomConfig {
                record_type: DnsRecordType::parse("CNAME").unwrap(),
                domain: "example.com".into(),
                host: "app".into(),
                answer: "old.example.com".into(),
            },
            id: 42,
        }];
        let new = vec![NamecomConfig {
            record_type: DnsRecordType::parse("A").unwrap(),
            domain: "example.com".into(),
            host: "app".into(),
            answer: "1.2.3.4".into(),
        }];

        let applied = namecom_update_with_base(&format!("http://{addr}"), Some(&creds), &old, &new)
            .await
            .unwrap();

        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].id, 99);

        let st = state.lock().await;
        assert_eq!(st.deleted, vec![42]);
        assert_eq!(
            st.created,
            vec![("A".into(), "app".into(), "1.2.3.4".into())]
        );
    }
}
