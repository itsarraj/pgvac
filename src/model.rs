use chrono::{DateTime, Utc};
use serde::Serialize;

/// One row of `pg_stat_user_tables`, trimmed to the columns this tool
/// actually reasons about. Everything here is exactly what Postgres
/// reports — no derived fields.
#[derive(Debug, Clone, PartialEq)]
pub struct TableStat {
    pub schema: String,
    pub table: String,
    pub n_live_tup: i64,
    pub n_dead_tup: i64,
    pub n_mod_since_analyze: i64,
    pub last_vacuum: Option<DateTime<Utc>>,
    pub last_autovacuum: Option<DateTime<Utc>>,
    pub last_analyze: Option<DateTime<Utc>>,
    pub last_autoanalyze: Option<DateTime<Utc>>,
    pub vacuum_count: i64,
    pub autovacuum_count: i64,
}

impl TableStat {
    /// The more recent of a manual `VACUUM` and an autovacuum run, or
    /// `None` if neither has ever happened. Postgres tracks these as two
    /// separate timestamp columns; a table freshly cleaned by autovacuum
    /// still counts as "recently vacuumed" even if it's never once been
    /// vacuumed by hand.
    pub fn last_vacuumed_at(&self) -> Option<DateTime<Utc>> {
        match (self.last_vacuum, self.last_autovacuum) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    /// `n_dead_tup / n_live_tup`, the ratio autovacuum's own
    /// `autovacuum_vacuum_scale_factor` setting is compared against. A
    /// table with zero live rows but some dead ones (fully deleted, never
    /// reused) reports a ratio of `f64::INFINITY` rather than dividing by
    /// zero into a nonsense number or panicking — infinity sorts and
    /// compares correctly against any finite threshold, which is exactly
    /// the "yes, this needs attention" answer.
    pub fn dead_ratio(&self) -> f64 {
        if self.n_live_tup == 0 {
            if self.n_dead_tup == 0 {
                0.0
            } else {
                f64::INFINITY
            }
        } else {
            self.n_dead_tup as f64 / self.n_live_tup as f64
        }
    }

    pub fn ever_vacuumed(&self) -> bool {
        self.last_vacuum.is_some() || self.last_autovacuum.is_some()
    }
}

/// Thresholds a table is judged against. All configurable from the CLI.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    pub dead_ratio: f64,
    pub min_dead_tuples: i64,
    pub stale_hours: Option<f64>,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            dead_ratio: 0.20,
            min_dead_tuples: 50,
            stale_hours: None,
        }
    }
}

/// Why a table was flagged. Kept as a real enum (not a formatted string)
/// so unit tests can assert on which specific condition fired, rather than
/// pattern-matching rendered text.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind")]
pub enum Reason {
    /// `dead_ratio >= threshold` and there are enough dead rows in
    /// absolute terms for the ratio to mean something (see
    /// `min_dead_tuples` — a 2-row table going from 1 to 2 dead rows is a
    /// 100% ratio and a complete non-event).
    HighDeadRatio { ratio: f64, threshold: f64 },
    /// Never vacuumed (autovacuum included) despite holding dead rows —
    /// the table isn't merely overdue, autovacuum has plausibly never
    /// even run on it once (a common `autovacuum = off` on the table
    /// itself, not just a lull).
    NeverVacuumed { n_dead_tup: i64 },
    /// Vacuumed at some point, but longer ago than `stale_hours`, and
    /// still holding at least `min_dead_tuples` dead rows.
    StaleVacuum { hours_since: f64, threshold: f64 },
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reason::HighDeadRatio { ratio, threshold } => {
                if ratio.is_infinite() {
                    write!(f, "dead ratio infinite (>= {threshold:.0}% threshold)")
                } else {
                    write!(
                        f,
                        "dead ratio {:.1}% >= {:.0}% threshold",
                        ratio * 100.0,
                        threshold * 100.0
                    )
                }
            }
            Reason::NeverVacuumed { n_dead_tup } => {
                write!(f, "never vacuumed, {n_dead_tup} dead tuples accumulated")
            }
            Reason::StaleVacuum {
                hours_since,
                threshold,
            } => {
                write!(
                    f,
                    "last vacuum {hours_since:.1}h ago >= {threshold:.0}h threshold"
                )
            }
        }
    }
}

/// The verdict for one table: its ratio plus the reasons (if any) it was
/// flagged. `reasons` is empty for a healthy table — `flagged` is just
/// `!reasons.is_empty()`, kept as its own field so callers don't need to
/// know that.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    pub dead_ratio: f64,
    pub flagged: bool,
    pub reasons: Vec<Reason>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub table: ReportTable,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReportTable {
    pub schema: String,
    pub table: String,
    pub n_live_tup: i64,
    pub n_dead_tup: i64,
    pub last_vacuumed_at: Option<DateTime<Utc>>,
}
