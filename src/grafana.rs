use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use tracing::*;

use super::clickhouse::ChClient;
use super::variables::VariablesAssignment;
use crate::variables;

#[derive(Debug, Default, Deserialize)]
pub struct VariablesConfig(HashMap<String, Vec<String>>);
impl VariablesConfig {
    pub fn check(&self, dashboard: &Dashboard) -> anyhow::Result<()> {
        if !self.0.is_empty() {
            warn!(config=?self.0, "Using variables configuration");
        }
        let variables: HashSet<&String> = dashboard.variables().map(|v| &v.name).collect();
        for var in self.0.keys() {
            if !variables.contains(var) {
                warn!(
                    "Variable {} does not exist in the dashboard (variables: {:?})",
                    var, variables
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct DashboardResponse {
    pub dashboard: Dashboard,
}
// See https://grafana.com/docs/grafana/latest/dashboards/build-dashboards/view-dashboard-json-model/
// TODO: Support all the fields.
#[derive(Debug, Deserialize)]
pub struct Dashboard {
    pub title: String,
    pub panels: Vec<Panel>,
    templating: TemplateList,
}
impl Dashboard {
    pub fn variables(&self) -> impl DoubleEndedIterator<Item = &Variable> {
        self.templating.list.iter()
    }
    pub fn variables_sql(&self) -> impl Iterator<Item = &Variable> {
        self.variables().filter(|v| v.is_clickhouse_ds())
    }
    // This is a bit inefficient, to be able to handle interdependent variables.
    pub async fn variables_combinations(
        &self,
        variables_config: VariablesConfig,
        client: &ChClient,
    ) -> anyhow::Result<Vec<VariablesAssignment<'_>>> {
        info!("Determining variables combinations");
        let progress = indicatif::ProgressBar::with_draw_target(
            Some(self.variables().count() as u64),
            indicatif::ProgressDrawTarget::hidden(),
        );
        let mut combinations: Vec<VariablesAssignment> = vec![Default::default()];
        for var in self.variables() {
            info!(
                var.name,
                "Processing variable {}/{}, ETA {}",
                progress.position(),
                progress.length().unwrap(),
                indicatif::HumanDuration(progress.eta())
            );
            let mut combinations2 = Vec::<VariablesAssignment>::default();
            for assignment in &combinations {
                // WARN: This heavily relies on the caching in the Clickhouse client to not rerun
                // the queries that have no dependency in some variables
                let variants: Box<dyn Iterator<Item = String>> =
                    // TODO: Would be nice to avoid the clone
                    if let Some(variants) = variables_config.0.get(&var.name).cloned() {
                        // NOTE: It could also make sense to skip the ones that are not part of the
                        // query response.
                        Box::new(variants.into_iter())
                    } else {
                        Box::new(var.get_variants(client, assignment).await?)
                    };
                for val in variants {
                    let mut assignment2 = assignment.clone();
                    assignment2.insert(var.name.as_str(), val);
                    combinations2.push(assignment2);
                }
            }
            combinations = combinations2;
            progress.inc(1);
        }
        Ok(combinations)
    }
}

#[derive(Debug, Deserialize)]
struct TemplateList {
    list: Vec<Variable>,
}
#[derive(Debug, Deserialize)]
pub struct Variable {
    pub name: String,
    pub query: String,
    #[serde(default)]
    options: Vec<VariableOption>,
    datasource: Option<DataSource>,
}

#[derive(Debug, Deserialize)]
struct VariableOption {
    value: String,
}

#[derive(Debug, Deserialize)]
struct DataSource {
    r#type: String,
}
#[derive(Debug, Deserialize)]
pub struct Panel {
    #[serde(default)]
    pub title: String,
    pub id: u64,
    #[serde(default)]
    targets: Vec<Target>,
    pub r#type: String,
}
impl std::fmt::Display for Panel {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Panel {} ({})", self.id, self.title)
    }
}

impl Panel {
    pub fn sql(&self) -> impl Iterator<Item = &String> {
        self.targets.iter().flat_map(|t| &t.raw_sql)
    }
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    raw_sql: Option<String>,
}

impl Variable {
    fn is_clickhouse_ds(&self) -> bool {
        self.datasource
            .as_ref()
            .map_or(false, |ds| ds.r#type.contains("clickhouse"))
    }
    #[tracing::instrument(skip_all,fields(variable=self.name) )]
    async fn get_variants(
        &self,
        client: &ChClient,
        variables: &VariablesAssignment<'_>,
    ) -> anyhow::Result<Box<dyn Iterator<Item = String> + '_>> {
        match &self.datasource {
            Some(_) if self.is_clickhouse_ds() => {
                let query = variables::substitute_variables(&self.query, variables)?;
                trace!(query, "Handling Clickhouse query variable");

                // The trick is to enable caching to not re-run queries that are equivalent after
                // substitution. With more effort, we could notice this before the substitution.
                let resp = client.query(query, true).await?;

                // For caching. It is a bit wasteful we have to do the query twice, but Grafana
                // uses the native protocol, which is harder to parse.
                client.query_native(self.query.clone()).await?;

                anyhow::ensure!(resp.first().map(|r| r.n_cols()).unwrap_or_default() <= 1);
                Ok(Box::new(resp.into_iter().flat_map(|c| c.cols)))
            }
            None => {
                trace!(var = self.query, "Handling JSON variable");
                Ok(Box::new(self.options.iter().map(|o| o.value.clone())))
            }
            _ => {
                anyhow::bail!("Unsupported variable data source {:?}", self);
            }
        }
    }
}
