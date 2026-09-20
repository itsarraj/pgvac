//! The actual judgment call, kept pure and database-free so it's fully
//! unit-testable: given a table's stats, thresholds, and "now", decide
//! whether it needs manual attention and why.

use chrono::{DateTime, Utc};

use crate::model::{Reason, TableStat, Thresholds, Verdict};

pub fn evaluate(stat: &TableStat, thresholds: &Thresholds, now: DateTime<Utc>) -> Verdict {
    let ratio = stat.dead_ratio();
    let mut reasons = Vec::new();

    let enough_dead_rows_to_care = stat.n_dead_tup >= thresholds.min_dead_tuples;

    if enough_dead_rows_to_care && ratio >= thresholds.dead_ratio {
        reasons.push(Reason::HighDeadRatio {
            ratio,
            threshold: thresholds.dead_ratio,
        });
    }

    if enough_dead_rows_to_care && !stat.ever_vacuumed() {
        reasons.push(Reason::NeverVacuumed {
            n_dead_tup: stat.n_dead_tup,
        });
    } else if let Some(stale_threshold) = thresholds.stale_hours {
        if enough_dead_rows_to_care {
            if let Some(last) = stat.last_vacuumed_at() {
                let hours_since = (now - last).num_seconds() as f64 / 3600.0;
                if hours_since >= stale_threshold {
                    reasons.push(Reason::StaleVacuum {
                        hours_since,
                        threshold: stale_threshold,
                    });
                }
            }
        }
    }

    Verdict {
        dead_ratio: ratio,
        flagged: !reasons.is_empty(),
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn base(now: DateTime<Utc>) -> TableStat {
        TableStat {
            schema: "public".into(),
            table: "t".into(),
            n_live_tup: 1000,
            n_dead_tup: 0,
            n_mod_since_analyze: 0,
            last_vacuum: None,
            last_autovacuum: Some(now),
            last_analyze: None,
            last_autoanalyze: Some(now),
            vacuum_count: 0,
            autovacuum_count: 1,
        }
    }

    #[test]
    fn healthy_table_not_flagged() {
        let now = Utc::now();
        let stat = base(now);
        let v = evaluate(&stat, &Thresholds::default(), now);
        assert!(!v.flagged);
        assert!(v.reasons.is_empty());
        assert_eq!(v.dead_ratio, 0.0);
    }

    #[test]
    fn high_dead_ratio_flagged() {
        let now = Utc::now();
        let mut stat = base(now);
        stat.n_dead_tup = 500; // 50% of 1000 live
        let v = evaluate(&stat, &Thresholds::default(), now);
        assert!(v.flagged);
        assert!(matches!(
            v.reasons[0],
            Reason::HighDeadRatio { ratio, .. } if (ratio - 0.5).abs() < 1e-9
        ));
    }

    #[test]
    fn high_ratio_but_below_min_dead_tuples_not_flagged() {
        // 3 dead out of 10 live = 30% ratio, well past the 20% default
        // threshold, but only 3 absolute dead rows — exactly the "tiny
        // table, meaningless ratio" case min_dead_tuples exists to filter.
        let now = Utc::now();
        let mut stat = base(now);
        stat.n_live_tup = 10;
        stat.n_dead_tup = 3;
        let v = evaluate(&stat, &Thresholds::default(), now);
        assert!(!v.flagged);
    }

    #[test]
    fn zero_live_rows_with_dead_rows_is_infinite_ratio_and_flagged() {
        let now = Utc::now();
        let mut stat = base(now);
        stat.n_live_tup = 0;
        stat.n_dead_tup = 200;
        let v = evaluate(&stat, &Thresholds::default(), now);
        assert!(v.dead_ratio.is_infinite());
        assert!(v.flagged);
    }

    #[test]
    fn zero_live_zero_dead_is_zero_ratio_not_infinite() {
        let now = Utc::now();
        let mut stat = base(now);
        stat.n_live_tup = 0;
        stat.n_dead_tup = 0;
        let v = evaluate(&stat, &Thresholds::default(), now);
        assert_eq!(v.dead_ratio, 0.0);
        assert!(!v.flagged);
    }

    #[test]
    fn never_vacuumed_with_dead_rows_flagged() {
        let now = Utc::now();
        let mut stat = base(now);
        stat.last_vacuum = None;
        stat.last_autovacuum = None;
        stat.n_dead_tup = 100; // above default min_dead_tuples (50)
        let v = evaluate(&stat, &Thresholds::default(), now);
        assert!(v.flagged);
        assert!(v
            .reasons
            .iter()
            .any(|r| matches!(r, Reason::NeverVacuumed { n_dead_tup: 100 })));
    }

    #[test]
    fn never_vacuumed_but_too_few_dead_rows_not_flagged() {
        let now = Utc::now();
        let mut stat = base(now);
        stat.last_vacuum = None;
        stat.last_autovacuum = None;
        stat.n_dead_tup = 5; // below default min_dead_tuples
        let v = evaluate(&stat, &Thresholds::default(), now);
        assert!(!v.flagged);
    }

    #[test]
    fn stale_vacuum_flagged_when_threshold_set() {
        let now = Utc::now();
        let mut stat = base(now);
        stat.last_autovacuum = Some(now - Duration::hours(48));
        stat.n_dead_tup = 100;
        // Ratio is 10%, below the 20% default, so only staleness should
        // trip this — proves the two checks are independent.
        let thresholds = Thresholds {
            stale_hours: Some(24.0),
            ..Thresholds::default()
        };
        let v = evaluate(&stat, &thresholds, now);
        assert!(v.flagged);
        assert!(v.reasons.iter().any(|r| matches!(
            r,
            Reason::StaleVacuum { hours_since, threshold: 24.0 } if (*hours_since - 48.0).abs() < 0.01
        )));
    }

    #[test]
    fn recent_vacuum_not_flagged_as_stale() {
        let now = Utc::now();
        let mut stat = base(now);
        stat.last_autovacuum = Some(now - Duration::hours(1));
        stat.n_dead_tup = 100;
        let thresholds = Thresholds {
            stale_hours: Some(24.0),
            ..Thresholds::default()
        };
        let v = evaluate(&stat, &thresholds, now);
        assert!(!v.flagged);
    }

    #[test]
    fn stale_check_disabled_by_default() {
        let now = Utc::now();
        let mut stat = base(now);
        stat.last_autovacuum = Some(now - Duration::days(365));
        stat.n_dead_tup = 100; // ratio only 10%, under default 20% threshold
        let v = evaluate(&stat, &Thresholds::default(), now);
        assert!(!v.flagged);
    }

    #[test]
    fn never_vacuumed_takes_priority_over_stale_check() {
        // With both a never-vacuumed table and a stale_hours threshold
        // set, only NeverVacuumed should fire — there's no last-vacuumed
        // timestamp to compute staleness from in the first place.
        let now = Utc::now();
        let mut stat = base(now);
        stat.last_vacuum = None;
        stat.last_autovacuum = None;
        stat.n_dead_tup = 100;
        let thresholds = Thresholds {
            stale_hours: Some(24.0),
            ..Thresholds::default()
        };
        let v = evaluate(&stat, &thresholds, now);
        assert_eq!(v.reasons.len(), 1);
        assert!(matches!(v.reasons[0], Reason::NeverVacuumed { .. }));
    }

    #[test]
    fn manual_vacuum_counts_same_as_autovacuum() {
        let now = Utc::now();
        let mut stat = base(now);
        stat.last_autovacuum = None;
        stat.last_vacuum = Some(now - Duration::hours(1));
        stat.n_dead_tup = 100;
        assert!(stat.ever_vacuumed());
        let v = evaluate(&stat, &Thresholds::default(), now);
        assert!(!v.flagged); // ratio only 10%, and it has been vacuumed
    }

    #[test]
    fn last_vacuumed_at_picks_the_more_recent_of_the_two() {
        let now = Utc::now();
        let mut stat = base(now);
        stat.last_vacuum = Some(now - Duration::hours(10));
        stat.last_autovacuum = Some(now - Duration::hours(1));
        assert_eq!(stat.last_vacuumed_at(), Some(now - Duration::hours(1)));
    }

    #[test]
    fn both_high_ratio_and_stale_can_fire_together() {
        let now = Utc::now();
        let mut stat = base(now);
        stat.last_autovacuum = Some(now - Duration::hours(100));
        stat.n_dead_tup = 600; // 60% ratio: above default 20%
        let thresholds = Thresholds {
            stale_hours: Some(24.0),
            ..Thresholds::default()
        };
        let v = evaluate(&stat, &thresholds, now);
        assert_eq!(v.reasons.len(), 2);
    }
}
