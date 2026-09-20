use chrono::Utc;

use crate::analyze::evaluate;
use crate::model::{Report, ReportTable, TableStat, Thresholds, Verdict};

/// Build one `Report` per table: the read-only facts plus the verdict.
/// Pure with respect to "now" so tests can pin it.
pub fn build_reports(
    stats: &[TableStat],
    thresholds: &Thresholds,
    now: chrono::DateTime<Utc>,
) -> Vec<Report> {
    stats
        .iter()
        .map(|s| Report {
            table: ReportTable {
                schema: s.schema.clone(),
                table: s.table.clone(),
                n_live_tup: s.n_live_tup,
                n_dead_tup: s.n_dead_tup,
                last_vacuumed_at: s.last_vacuumed_at(),
            },
            verdict: evaluate(s, thresholds, now),
        })
        .collect()
}

fn fmt_ratio(v: &Verdict) -> String {
    if v.dead_ratio.is_infinite() {
        "inf".to_string()
    } else {
        format!("{:.1}%", v.dead_ratio * 100.0)
    }
}

/// Human-readable table, flagged rows first, then flagged reasons listed
/// underneath each one. `show_all` includes healthy tables too; without it
/// only flagged tables are printed (the "what needs my attention right
/// now" default).
pub fn render_text(reports: &[Report], show_all: bool) -> String {
    let mut out = String::new();
    let flagged: Vec<&Report> = reports.iter().filter(|r| r.verdict.flagged).collect();
    let healthy_count = reports.len() - flagged.len();

    out.push_str(&format!(
        "{} table(s) checked, {} flagged, {} healthy\n",
        reports.len(),
        flagged.len(),
        healthy_count
    ));

    if flagged.is_empty() {
        out.push_str("no tables need manual vacuum attention\n");
    } else {
        out.push_str("\nNEEDS ATTENTION\n");
        for r in &flagged {
            out.push_str(&format!(
                "  {}.{}  live={} dead={} ratio={}\n",
                r.table.schema,
                r.table.table,
                r.table.n_live_tup,
                r.table.n_dead_tup,
                fmt_ratio(&r.verdict)
            ));
            for reason in &r.verdict.reasons {
                out.push_str(&format!("      - {reason}\n"));
            }
        }
    }

    if show_all {
        let healthy: Vec<&Report> = reports.iter().filter(|r| !r.verdict.flagged).collect();
        if !healthy.is_empty() {
            out.push_str("\nHEALTHY\n");
            for r in healthy {
                out.push_str(&format!(
                    "  {}.{}  live={} dead={} ratio={}\n",
                    r.table.schema,
                    r.table.table,
                    r.table.n_live_tup,
                    r.table.n_dead_tup,
                    fmt_ratio(&r.verdict)
                ));
            }
        }
    }

    out
}

pub fn render_json(reports: &[Report]) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TableStat;

    fn stat(schema: &str, table: &str, live: i64, dead: i64) -> TableStat {
        TableStat {
            schema: schema.into(),
            table: table.into(),
            n_live_tup: live,
            n_dead_tup: dead,
            n_mod_since_analyze: 0,
            last_vacuum: None,
            last_autovacuum: Some(Utc::now()),
            last_analyze: None,
            last_autoanalyze: Some(Utc::now()),
            vacuum_count: 0,
            autovacuum_count: 1,
        }
    }

    #[test]
    fn text_output_lists_only_flagged_by_default() {
        let stats = vec![
            stat("public", "clean", 1000, 5),
            stat("public", "dirty", 100, 90),
        ];
        let reports = build_reports(&stats, &Thresholds::default(), Utc::now());
        let text = render_text(&reports, false);
        assert!(text.contains("dirty"));
        assert!(!text.contains("clean"));
    }

    #[test]
    fn text_output_show_all_includes_healthy_section() {
        let stats = vec![
            stat("public", "clean", 1000, 5),
            stat("public", "dirty", 100, 90),
        ];
        let reports = build_reports(&stats, &Thresholds::default(), Utc::now());
        let text = render_text(&reports, true);
        assert!(text.contains("HEALTHY"));
        assert!(text.contains("clean"));
    }

    #[test]
    fn json_output_round_trips_flag_state() {
        let stats = vec![stat("public", "dirty", 100, 90)];
        let reports = build_reports(&stats, &Thresholds::default(), Utc::now());
        let json = render_json(&reports).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0]["verdict"]["flagged"], true);
        assert_eq!(parsed[0]["table"]["table"], "dirty");
    }

    #[test]
    fn no_flagged_tables_prints_reassuring_message() {
        let stats = vec![stat("public", "clean", 1000, 5)];
        let reports = build_reports(&stats, &Thresholds::default(), Utc::now());
        let text = render_text(&reports, false);
        assert!(text.contains("no tables need manual vacuum attention"));
    }

    #[test]
    fn infinite_ratio_renders_as_inf_not_a_crash() {
        let stats = vec![stat("public", "wiped", 0, 500)];
        let reports = build_reports(&stats, &Thresholds::default(), Utc::now());
        let text = render_text(&reports, false);
        assert!(text.contains("ratio=inf"));
    }
}
