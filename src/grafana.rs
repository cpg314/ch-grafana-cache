use std::collections::HashMap;

use serde::Deserialize;
use tracing::*;

use super::clickhouse::ChClient;
use super::variables::VariablesAssignment;
use crate::variables;

#[derive(Debug, Deserialize)]
pub struct DashboardResponse {
    pub dashboard: Dashboard,
}
#[derive(Debug, Deserialize)]
pub struct Dashboard {
    pub title: String,
    panels: Vec<Panel>,
    templating: TemplateList,
}
impl Dashboard {
    pub fn variables(&self) -> impl DoubleEndedIterator<Item = &Variable> {
        self.templating.list.iter()
    }
    pub fn variables_sql(&self) -> impl Iterator<Item = &String> {
        self.variables()
            .filter(|v| v.is_clickhouse_ds())
            .map(|ds| &ds.query)
    }
    pub fn panels_sql(&self) -> impl Iterator<Item = &String> {
        self.panels
            .iter()
            .flat_map(|p| &p.targets)
            .flat_map(|t| &t.raw_sql)
    }
    // This is a bit inefficient, to be able to handle interdependent variables.
    pub async fn variables_combinations(
        &self,
        client: &ChClient,
    ) -> anyhow::Result<Vec<VariablesAssignment<'_>>> {
        let mut combinations = Vec::<VariablesAssignment>::default();
        for (i, var) in self.variables().enumerate() {
            if i == 0 {
                combinations = var
                    .get_variants(client, &Default::default())
                    .await?
                    .map(|val| HashMap::from([(var.name.as_str(), val)]))
                    .collect();
            } else {
                let mut combinations2 = Vec::<VariablesAssignment>::default();
                for assignment in &combinations {
                    // WARN: This heavily relies on the caching in the Clickhouse client
                    for val in var.get_variants(client, assignment).await? {
                        let mut assignment2 = assignment.clone();
                        assignment2.insert(var.name.as_str(), val);
                        combinations2.push(assignment2);
                    }
                }
                combinations = combinations2;
            }
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
    query: String,
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
struct Panel {
    #[serde(default)]
    targets: Vec<Target>,
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
