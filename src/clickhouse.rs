use std::collections::HashMap;
use std::sync::Arc;

use tracing::*;

#[derive(clap::Parser)]
pub struct Flags {
    /// URL to the Clickhouse HTTP endpoint
    #[clap(long, env = "CLICKHOUSE_URL")]
    url: reqwest::Url,
    /// Clickhouse username
    #[clap(long, env = "CLICKHOUSE_USERNAME")]
    username: String,
    #[clap(long, env = "CLICKHOUSE_PASSWORD")]
    password: Option<String>,
}

pub struct ChClient {
    builder: reqwest_middleware::RequestBuilder,
    cache: Arc<tokio::sync::Mutex<HashMap<String, Vec<ResultRow>>>>,
}
impl Clone for ChClient {
    fn clone(&self) -> Self {
        Self {
            builder: self.builder.try_clone().unwrap(),
            cache: self.cache.clone(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct ResultRow {
    pub cols: Vec<String>,
}
impl ResultRow {
    pub fn n_cols(&self) -> usize {
        self.cols.len()
    }
}

impl ChClient {
    pub fn from_flags(flags: &Flags) -> Self {
        let retry_policy =
            reqwest_retry::policies::ExponentialBackoff::builder().build_with_max_retries(3);
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(reqwest_retry::RetryTransientMiddleware::new_with_policy(
                retry_policy,
            ))
            .build();
        ChClient {
            builder: client
                .get(flags.url.clone())
                .basic_auth(&flags.username, flags.password.clone()),
            cache: Default::default(),
        }
    }
    #[instrument(skip(self))]
    pub async fn query_native(&self, query: String) -> anyhow::Result<u64> {
        debug!("Sending query");
        Ok(self
            .clone()
            .builder
            .query(&[("default_format", "Native")])
            .body(query)
            .send()
            .await?
            .error_for_status()?
            .content_length()
            .unwrap_or_default())
    }
    #[instrument(skip(self))]
    pub async fn query(&self, query: String, cache: bool) -> anyhow::Result<Vec<ResultRow>> {
        if cache {
            let cache = self.cache.lock().await;
            if let Some(resp) = cache.get(&query) {
                return Ok(resp.clone());
            }
        }
        debug!("Sending query");
        let resp = self.clone().builder.body(query.clone()).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "{}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }
        let hit = resp.headers().get("x-cache").map_or(false, |c| c == "HIT");
        trace!(hit, "Received response");
        let rows: Vec<ResultRow> = resp
            .text()
            .await?
            .lines()
            .map(|l| ResultRow {
                cols: l.split('\t').map(String::from).collect(),
            })
            .collect();
        if !rows.is_empty() {
            anyhow::ensure!(
                rows.iter().all(|r| r.n_cols() == rows[0].n_cols()),
                "Inconsistent column sizes"
            );
        }
        if cache {
            let mut cache = self.cache.lock().await;
            cache.insert(query.clone(), rows.clone());
        }
        Ok(rows)
    }
}
