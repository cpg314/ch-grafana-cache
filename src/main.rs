mod clickhouse;
mod grafana;
mod variables;

use clap::Parser;
use tracing::*;

/// Execute Clickhouse SQL queries from a Grafana dashboard.
#[derive(clap::Parser)]
struct Flags {
    /// Base Grafana URL
    #[clap(long)]
    grafana: reqwest::Url,
    /// Grafana dashboard id
    #[clap(long)]
    dashboard: String,
    /// Synctect for syntax highlighting
    #[clap(long, env = "CH_GRAFANA_CACHE_THEME")]
    theme: Option<String>,
    #[clap(subcommand)]
    command: Command,
}
#[derive(clap::Parser)]
enum Command {
    /// Print SQL statements, with syntax highlighting
    Print,
    /// Execute the queries
    Execute {
        #[clap(flatten)]
        flags: clickhouse::Flags,
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

    info!("Retrieving dashboard");
    let resp: grafana::DashboardResponse = reqwest::get(
        args.grafana
            .join("api/dashboards/uid/")?
            .join(&args.dashboard)?,
    )
    .await?
    .json()
    .await?;
    let dashboard = &resp.dashboard;
    info!("Retrieved dashboard '{}'", dashboard.title);

    match args.command {
        Command::Print => {
            info!("Variables");
            for sql in dashboard.variables_sql() {
                print_sql(sql, args.theme.as_ref())?;
            }
            info!("Panels");
            for sql in dashboard.panels_sql() {
                print_sql(sql, args.theme.as_ref())?;
            }
        }
        Command::Execute { flags: ch_args } => {
            let client = clickhouse::ChClient::from_flags(&ch_args);

            let combinations = dashboard.variables_combinations(&client).await?;

            let n_combinations = combinations.len();
            info!(
                "{} variables combinations found. Executing queries...",
                n_combinations
            );

            for (i, combination) in combinations.into_iter().enumerate() {
                info!(i, n_combinations, "Executing combination");
                let start = std::time::Instant::now();
                debug!(?combination);

                let mut size = 0;
                for sql in dashboard.panels_sql() {
                    let sql = variables::substitute_variables(sql, &combination)?;
                    size += client.query_native(sql).await?;
                }
                info!(duration=?start.elapsed(), size_mb = size as f64/1e6, "Executed combination");
            }
            info!(duration=?start.elapsed(), "Done");
        }
    }

    Ok(())
}
