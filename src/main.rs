mod clickhouse;
mod grafana;
mod variables;

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use colored::Colorize;
use itertools::Itertools;
use tracing::*;

use grafana::VariablesConfig;

lazy_static::lazy_static! {
    static ref THEMES: Vec<String> =
        bat::assets::HighlightingAssets::from_binary().themes().map(String::from).collect();
}

/// Execute Clickhouse SQL queries from a Grafana dashboard.
///
/// Call with either --grafana-url and --dashboard, or with --json
#[derive(clap::Parser)]
#[clap(version)]
struct Flags {
    /// Base Grafana URL
    #[clap(long, env = "GRAFANA_URL")]
    grafana_url: Option<reqwest::Url>,
    /// Grafana dashboard id
    #[clap(long, requires = "grafana")]
    dashboard: Option<String>,
    /// Dashboard JSON file.
    #[clap(long, conflicts_with = "dashboard")]
    json: Option<PathBuf>,
    /// Synctect theme for syntax highlighting
    #[clap(long, env = "CH_GRAFANA_CACHE_THEME",
           value_parser=clap::builder::PossibleValuesParser::new(THEMES.iter().map(|s| s.as_str())))]
    theme: Option<String>,
    #[clap(subcommand)]
    command: Command,
}
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Json {
    Resp(grafana::DashboardResponse),
    Dashboard(grafana::Dashboard),
}
impl From<Json> for grafana::Dashboard {
    fn from(js: Json) -> grafana::Dashboard {
        match js {
            Json::Resp(r) => r.dashboard,
            Json::Dashboard(d) => d,
        }
    }
}
impl Flags {
    async fn get_dashboard(&self) -> anyhow::Result<grafana::Dashboard> {
        let resp: Json = match (&self.json, &self.grafana_url, &self.dashboard) {
            (Some(json), _, _) => serde_json::from_str(&std::fs::read_to_string(json)?)?,
            (None, Some(grafana), Some(dashboard)) => {
                info!("Retrieving dashboard from {}", grafana);
                reqwest::get(grafana.join("api/dashboards/uid/")?.join(dashboard)?)
                    .await?
                    .json::<Json>()
                    .await?
            }
            _ => {
                anyhow::bail!("Use --json, or --grafana and --dashboard")
            }
        };
        let dashboard = grafana::Dashboard::from(resp);
        Ok(dashboard)
    }
}

#[derive(clap::Parser)]
enum Command {
    /// Print SQL statements, with syntax highlighting
    Print,
    /// Execute the queries
    Execute {
        #[clap(flatten)]
        flags: clickhouse::Flags,
        /// YAML file of the form variable_name: [ values ] to manually specify the values of some
        /// variables in the dashboard
        #[clap(long)]
        variables_yaml: Option<PathBuf>,
    },
}
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    if let Err(e) = main_impl().await {
        error!("{:?}", e);
        std::process::exit(1);
    }
}
fn print_sql(sql: &str, theme: Option<&String>) -> anyhow::Result<()> {
    let sql = std::io::Cursor::new(sql.trim());
    let mut printer = bat::PrettyPrinter::new();
    printer.input_from_reader(sql).language("sql");
    if let Some(theme) = theme {
        printer.theme(theme);
    }
    printer.print()?;
    println!("\n",);
    Ok(())
}
async fn main_impl() -> anyhow::Result<()> {
    let args = Flags::parse();
    let start = std::time::Instant::now();

    let dashboard = args.get_dashboard().await?;
    info!(
        "Retrieved dashboard '{}' with variables {}",
        dashboard.title,
        dashboard.variables().map(|v| &v.name).join(", ")
    );
    match args.command {
        Command::Print => {
            println!();
            println!("{}", "Variables:\n".yellow().bold());
            for var in dashboard.variables_sql() {
                println!("{}", var.name.yellow());
                print_sql(&var.query, args.theme.as_ref())?;
            }
            println!("{}", "Panels:\n".yellow().bold());
            for panel in dashboard.panels {
                if panel.sql().next().is_some() {
                    println!("{}", panel.to_string().yellow());
                }
                for sql in panel.sql() {
                    print_sql(sql, args.theme.as_ref())?;
                }
            }
        }
        Command::Execute {
            flags: ch_args,
            variables_yaml,
        } => {
            let variables_config = if let Some(variables_yaml) = &variables_yaml {
                serde_yaml::from_str(
                    &std::fs::read_to_string(variables_yaml)
                        .with_context(|| format!("Could not open {:?}", variables_yaml))?,
                )?
            } else {
                VariablesConfig::default()
            };
            debug!(?variables_config);
            variables_config.check(&dashboard)?;
            let client = clickhouse::ChClient::from_flags(&ch_args);

            let combinations = dashboard
                .variables_combinations(variables_config, &client)
                .await?;

            let n_combinations = combinations.len();
            info!(
                "{} variables combinations found. Executing queries...",
                n_combinations
            );
            let progress = indicatif::ProgressBar::with_draw_target(
                Some(n_combinations as u64),
                indicatif::ProgressDrawTarget::hidden(),
            );
            for combination in combinations {
                info!(
                    ?combination,
                    "Executing combination {}/{}, ETA {}.",
                    progress.position(),
                    progress.length().unwrap(),
                    indicatif::HumanDuration(progress.eta())
                );
                let start = std::time::Instant::now();
                debug!(?combination);

                let mut size = 0;
                for panel in &dashboard.panels {
                    for sql in panel.sql() {
                        let sql = variables::substitute_variables(sql, &combination)?;
                        size += client.query_native(sql.clone()).await.with_context(|| {
                            format!("Failed to run query [{}] in panel {}", sql, panel)
                        })?;
                    }
                }
                info!(duration=?start.elapsed(), size_mb = size as f64/1e6, "Executed combination");
                progress.inc(1);
            }
            info!(duration=?start.elapsed(), "Done");
        }
    }

    Ok(())
}
