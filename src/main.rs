use anyhow::Result;
use chrono::Utc;
use clap::{Parser, ValueEnum};
use pgvac::model::Thresholds;
use pgvac::{db, render};

#[derive(Copy, Clone, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Parser)]
#[command(
    name = "pgvac",
    version,
    about = "Analyzes pg_stat_user_tables for autovacuum health and flags tables needing manual VACUUM attention"
)]
struct Cli {
    /// Connection string — `postgres://user@host:5432/dbname` or libpq
    /// keyword form (`host=... port=... dbname=...`).
    conninfo: String,

    /// Only check this schema. Default: every schema `pg_stat_user_tables`
    /// reports on (which already excludes system catalogs).
    #[arg(long)]
    schema: Option<String>,

    /// Flag a table once `n_dead_tup / n_live_tup` reaches this fraction.
    #[arg(long, default_value_t = 0.20)]
    dead_ratio: f64,

    /// Ignore ratio/staleness checks below this many absolute dead
    /// rows — without a floor, a 2-row table going from 1 to 2 dead rows
    /// is a "100% dead ratio" and a complete non-event.
    #[arg(long, default_value_t = 50)]
    min_dead_tuples: i64,

    /// Also flag a table whose last vacuum (manual or auto) is older than
    /// this many hours, provided it still holds `min_dead_tuples` dead
    /// rows. Off by default — many tables are legitimately vacuumed
    /// rarely because they rarely change.
    #[arg(long)]
    stale_hours: Option<f64>,

    /// Include healthy tables in the output, not just flagged ones.
    #[arg(long)]
    all: bool,

    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let thresholds = Thresholds {
        dead_ratio: cli.dead_ratio,
        min_dead_tuples: cli.min_dead_tuples,
        stale_hours: cli.stale_hours,
    };

    let mut client = db::connect(&cli.conninfo)?;
    let stats = db::fetch_stats(&mut client, cli.schema.as_deref())?;
    let reports = render::build_reports(&stats, &thresholds, Utc::now());

    match cli.format {
        Format::Text => print!("{}", render::render_text(&reports, cli.all)),
        Format::Json => println!("{}", render::render_json(&reports)?),
    }

    if reports.iter().any(|r| r.verdict.flagged) {
        std::process::exit(1);
    }
    Ok(())
}
