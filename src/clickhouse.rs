use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::StreamExt;
use reqwest::header::TRANSFER_ENCODING;
use tracing::*;

#[derive(clap::Parser)]
pub struct Flags {
    /// URL to the Clickhouse HTTP endpoint
    #[clap(long, env = "CLICKHOUSE_URL")]
    pub url: reqwest::Url,
    /// Clickhouse username
    #[clap(long, env = "CLICKHOUSE_USERNAME")]
    pub username: String,
    #[clap(long, env = "CLICKHOUSE_PASSWORD")]
    pub password: Option<String>,
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
#[derive(Default)]
pub struct QueryOutput {
    pub rows: usize,
    pub total_size: usize,
}
impl std::ops::AddAssign for QueryOutput {
    fn add_assign(&mut self, rhs: Self) {
        self.rows += rhs.rows;
        self.total_size += rhs.total_size;
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
                .post(flags.url.clone())
                .header(TRANSFER_ENCODING, "chunked")
                .basic_auth(&flags.username, flags.password.clone()),
            cache: Default::default(),
        }
    }
    pub async fn send_query(
        &self,
        query: String,
        default_format: &str,
    ) -> anyhow::Result<reqwest::Response> {
        debug!("Sending query");
        let resp = self
            .clone()
            .builder
            .query(&[("default_format", default_format)])
            .body(query.clone())
            .send()
            .await?;
        debug!("{:?}", resp.headers());
        if !resp.status().is_success() {
            anyhow::bail!(
                "{}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }
        Ok(resp)
    }
    #[instrument(skip(self))]
    pub async fn query_native(&self, query: String) -> anyhow::Result<QueryOutput> {
        let resp = self.send_query(query.clone(), "Native").await?;
        debug!("{:?}", resp.headers());
        let mut q = resp.bytes_stream();
        // NOTE: Not clear if we need to consume for the cache to succeed.
        // We could also use https://clickhouse.com/docs/en/interfaces/http#response-buffering
        let mut output = QueryOutput::default();
        while let Some(q) = q.next().await {
            output.total_size += q?.len();
            output.rows += 1;
        }

        Ok(output)
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
        let resp = self.send_query(query.clone(), "Native").await?;
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
