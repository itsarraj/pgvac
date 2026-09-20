//! The only part that touches a server: one read-only query against
//! `pg_stat_user_tables`, the same view `pg_activity`/autovacuum tuning
//! guides point at — nothing here writes, locks, or runs `VACUUM` itself.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use postgres::{Client, NoTls, Row};

use crate::model::TableStat;

const QUERY: &str = "
SELECT
    schemaname::text,
    relname::text,
    n_live_tup,
    n_dead_tup,
    n_mod_since_analyze,
    last_vacuum,
    last_autovacuum,
    last_analyze,
    last_autoanalyze,
    vacuum_count,
    autovacuum_count
FROM pg_stat_user_tables
WHERE ($1::text IS NULL OR schemaname = $1)
ORDER BY schemaname, relname
";

pub fn connect(conninfo: &str) -> Result<Client> {
    Client::connect(conninfo, NoTls).context("could not connect to Postgres")
}

fn row_to_stat(row: &Row) -> TableStat {
    TableStat {
        schema: row.get(0),
        table: row.get(1),
        n_live_tup: row.get(2),
        n_dead_tup: row.get(3),
        n_mod_since_analyze: row.get(4),
        last_vacuum: row.get::<_, Option<DateTime<Utc>>>(5),
        last_autovacuum: row.get::<_, Option<DateTime<Utc>>>(6),
        last_analyze: row.get::<_, Option<DateTime<Utc>>>(7),
        last_autoanalyze: row.get::<_, Option<DateTime<Utc>>>(8),
        vacuum_count: row.get(9),
        autovacuum_count: row.get(10),
    }
}

pub fn fetch_stats(client: &mut Client, schema: Option<&str>) -> Result<Vec<TableStat>> {
    let rows = client
        .query(QUERY, &[&schema])
        .context("failed to query pg_stat_user_tables")?;
    Ok(rows.iter().map(row_to_stat).collect())
}
