# pgvac

Reads `pg_stat_user_tables` on a live Postgres and flags tables autovacuum
hasn't kept up with — high dead-tuple ratio, tables that have literally
never been vacuumed despite holding dead rows, or (optionally) tables
that haven't been vacuumed in longer than you're comfortable with. The
underlying view already has everything needed for this; the gap is that
nobody runs the query until something is already slow, and the query
itself is easy to write wrong (dividing by zero on an empty table,
forgetting that `n_dead_tup` alone means nothing without a sense of
table size). `pgtop` in this same workspace watches *current activity*;
this watches the thing that quietly rots in the background if nothing's
watching it — table bloat from autovacuum falling behind.

## Installation

```sh
cargo install --path .
```

## Usage

```bash
pgvac "postgres://user@host:5432/mydb"                  # flagged tables only
pgvac "postgres://user@host:5432/mydb" --all             # every table, flagged or not
pgvac "$DATABASE_URL" --schema public                     # one schema

pgvac "$DATABASE_URL" --dead-ratio 0.3 --min-dead-tuples 200   # looser thresholds
pgvac "$DATABASE_URL" --stale-hours 24                          # also flag by vacuum age
pgvac "$DATABASE_URL" --format json | jq '.[] | select(.verdict.flagged)'
```

Exits `1` if anything is flagged, `0` otherwise — wire it into a cron job
or a CI health check without parsing the text output.

## What gets flagged, and why the floor matters

Three independent checks, any one of which flags a table:

1. **High dead ratio.** `n_dead_tup / n_live_tup >= --dead-ratio`
   (default 20%) — the same ratio autovacuum's own
   `autovacuum_vacuum_scale_factor` is compared against internally, just
   surfaced instead of buried in a background log line.
2. **Never vacuumed.** Both `last_vacuum` and `last_autovacuum` are
   null, but the table already holds dead rows — usually means
   `autovacuum_enabled = false` got set on that one table (deliberately
   or by accident) rather than a global settings problem.
3. **Stale vacuum** (`--stale-hours`, off by default). The most recent
   of `last_vacuum`/`last_autovacuum` is older than the threshold *and*
   the table still holds `--min-dead-tuples` dead rows. Off by default
   because plenty of tables are legitimately vacuumed rarely — they
   rarely change — and staleness alone, without dead rows to show for
   it, isn't a problem.

`--min-dead-tuples` (default 50) gates all three checks. Without it, a
2-row lookup table going from 1 dead row to 2 is a "100% dead ratio" and
a complete non-event; the floor exists specifically to stop that from
drowning out tables that actually matter. A table with zero live rows
and any dead rows reports its ratio as `inf` rather than dividing by
zero — that's still `>= any finite threshold`, so it flags correctly
instead of crashing or silently reporting `0%`.

## Status: built, and verified against real dead-tuple accumulation on a live Postgres, not synthetic numbers

- **19 unit tests** (`cargo test --lib`), no database needed for these:
  the ratio math (normal case, zero-live-zero-dead is `0.0` not `NaN`,
  zero-live-with-dead-rows is `inf` and still compares correctly against
  a finite threshold); the `min_dead_tuples` floor suppressing a
  high-percentage-but-tiny-absolute-count table; each of the three flag
  reasons firing independently and in combination (high ratio + stale
  vacuum together produces two reasons, not one overwriting the other);
  never-vacuumed taking priority over the staleness check when there's
  no last-vacuum timestamp to compute an age from in the first place;
  manual `VACUUM` counting the same as autovacuum for "has this table
  ever been vacuumed"; `last_vacuumed_at()` picking the more recent of
  the two; and the text/JSON renderers (flagged-only vs. `--all`,
  `inf` rendering as the literal string `inf` rather than panicking on
  a float formatter, JSON round-tripping the `flagged` boolean and
  table name).
- **Live-verified dead-tuple detection against a real Postgres**:
  created a throwaway database (`pgvac_test`), and two 2,000-row tables.
  Vacuumed both clean, then ran real `UPDATE`/`DELETE` churn against one
  of them (`autovacuum_enabled = false` set on it specifically, since
  this instance's autovacuum is fast enough to otherwise clean it up
  mid-test) — 1,500 updates, a 200-row delete, another 800 updates.
  **Checked `pg_stat_user_tables` directly with `psql` before running
  the tool at all**: `n_dead_tup` had genuinely gone from 0 to 2,500 on
  the churned table, 0 on the untouched one. Then ran `pgvac`:
  ```
  2 table(s) checked, 1 flagged, 1 healthy

  NEEDS ATTENTION
    public.churny  live=3800 dead=2500 ratio=65.8%
        - dead ratio 65.8% >= 20% threshold

  HEALTHY
    public.clean  live=2000 dead=0 ratio=0.0%
  ```
  Exit code was `1` (something flagged). The churned table was correctly
  flagged with the exact live/dead counts `psql` had just shown
  independently; the untouched, freshly-vacuumed table was correctly
  left alone. `--stale-hours 100` against the same database correctly
  did **not** add a second reason, since both tables had in fact been
  vacuumed (or created) within the last hour — proving the staleness
  check reads real timestamps rather than always firing once enabled.
  Against an unreachable port, `pgvac` printed `could not connect to
  Postgres: Connection refused (os error 111)` and exited `1` — no
  panic, no hang.
- **An honest thing this run surfaced, not a bug**: on the first churn
  attempt, autovacuum on this instance ran and cleaned the table up
  *during* the test, before the tool got to see the dead tuples — its
  scale factor was reached in under 15 seconds against a 2,000-row
  table. Re-ran with `autovacuum_enabled = false` set on the test table
  specifically so the dead tuples this test wanted to demonstrate would
  actually still be there. Worth knowing if you try to reproduce this
  by hand on a lightly-loaded database: autovacuum may beat you to it.

**Not done / deliberately deferred**: `pg_stat_user_tables` reports
dead-tuple counts as a running estimate updated by the stats collector,
not a live `COUNT(*)` — right after a burst of writes (as in the live
test above) `n_live_tup` can briefly overshoot the true row count until
the next `ANALYZE` corrects it. That's Postgres's own behavior, not
something this tool works around, and it's a reason to treat the ratio
as "needs a look" rather than an exact figure. Also out of scope: index
bloat (a related but separate problem `pg_stat_user_tables` doesn't
cover at all — see `pgstattuple`/`pg_repack` territory), toast table
statistics, `autovacuum_vacuum_scale_factor`/`_threshold` per-table
overrides (currently a global `--dead-ratio` from the CLI, not read from
each table's own `reloptions`), and running the actual `VACUUM` — this
only reports, on purpose, since a tool that goes on-call and starts
running `VACUUM FULL` on its own is a much scarier thing to be wrong
about than one that just prints a report.
