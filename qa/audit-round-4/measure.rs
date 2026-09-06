use rusqlite::{Connection, params, types::Value};
use serde_json::json;
use std::{fs, path::Path, time::Instant};

fn query(c: &Connection, sql: &str, from: &str, to: &str, lane: Option<i64>) -> Vec<Vec<Value>> {
    let mut stmt = c.prepare(sql).unwrap();
    let columns = stmt.column_count();
    stmt.query_map(params![from, to, lane, 200], |row| {
        (0..columns)
            .map(|i| row.get(i))
            .collect::<rusqlite::Result<Vec<Value>>>()
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}
fn plan(c: &Connection, sql: &str, from: &str, to: &str, lane: Option<i64>) -> Vec<String> {
    c.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap()
        .query_map(params![from, to, lane, 200], |r| r.get(3))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}
fn main() {
    let output = std::env::args().nth(1).expect("fixture output directory");
    let root = Path::new(&output);
    let queries = Path::new("qa/audit-round-4");
    let db = root.join("session-query-fixture.db");
    assert!(!db.exists(), "use a fresh, owned fixture path");
    let mut c = Connection::open(&db).unwrap();
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
        .unwrap();
    for name in [
        "0023_usage_ledger",
        "0024_usage_headline_raw",
        "0025_usage_headline_version",
        "0026_usage_subagents",
        "0027_usage_ingest_version",
        "0028_usage_events_model",
        "0029_usage_recount_failures",
    ] {
        c.execute_batch(
            &fs::read_to_string(format!("crates/repomon-core/migrations/{name}.sql")).unwrap(),
        )
        .unwrap();
    }
    let tx = c.transaction().unwrap();
    tx.execute_batch("WITH RECURSIVE seq(n) AS (VALUES(0) UNION ALL SELECT n+1 FROM seq WHERE n < 3599)
        INSERT INTO usage_sessions(agent_kind,session_id,headline,turns,tool_calls,retries)
        SELECT CASE WHEN n%2=0 THEN 'claude-code' ELSE 'codex' END, 's-'||n, 'Synthetic task '||n, 50, n%9, n%3 FROM seq;
        WITH RECURSIVE sessions(n) AS (VALUES(0) UNION ALL SELECT n+1 FROM sessions WHERE n < 3599),
        turns(t) AS (VALUES(0) UNION ALL SELECT t+1 FROM turns WHERE t < 49)
        INSERT INTO usage_events(at,agent_kind,model,account,lane_id,repo_id,session_id,cwd,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,thinking_tokens,estimated,external,subagent,source_path,source_offset)
        SELECT strftime('%Y-%m-%dT%H:%M:%fZ','2026-06-01', '+'||((n*53) % 129500 + t)||' minutes'),
            CASE WHEN n%2=0 THEN 'claude-code' ELSE 'codex' END, CASE WHEN t%3=0 THEN 'model-a' ELSE 'model-b' END,
            'fixture', CASE WHEN n%17=0 THEN NULL ELSE n%120 END, n%12, 's-'||n, '/synthetic/repo-'||(n%12),
            100+t, 30+t, 10, 5, t%7, n%11=0, n%17=0, t%5=0, '/synthetic/session-'||n, t
        FROM sessions CROSS JOIN turns;").unwrap();
    tx.commit().unwrap();
    c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    let before = fs::read_to_string(queries.join("session-before.sql")).unwrap();
    let filtered = fs::read_to_string(queries.join("session-filtered-after.sql")).unwrap();
    let all = fs::read_to_string(queries.join("session-all-after.sql")).unwrap();
    let mut results = Vec::new();
    for stats in [false, true] {
        if stats {
            c.execute_batch("ANALYZE").unwrap();
        }
        for (range, from, to) in [
            (
                "hour",
                "2026-08-01T00:00:00.000Z",
                "2026-08-01T01:00:00.000Z",
            ),
            (
                "month",
                "2026-08-01T00:00:00.000Z",
                "2026-09-01T00:00:00.000Z",
            ),
            (
                "history",
                "2026-06-01T00:00:00.000Z",
                "2026-09-01T00:00:00.000Z",
            ),
        ] {
            for lane in [None, Some(7), Some(999)] {
                let after = if lane.is_some() { &filtered } else { &all };
                let expected = query(&c, &before, from, to, lane);
                assert_eq!(expected, query(&c, after, from, to, lane));
                for _ in 0..3 {
                    query(&c, &before, from, to, lane);
                    query(&c, after, from, to, lane);
                }
                let mut samples = [Vec::new(), Vec::new()];
                for iteration in 0..21 {
                    for which in if iteration % 2 == 0 { [0, 1] } else { [1, 0] } {
                        let start = Instant::now();
                        let rows =
                            query(&c, if which == 0 { &before } else { after }, from, to, lane);
                        samples[which].push(start.elapsed().as_secs_f64() * 1000.0);
                        assert_eq!(expected, rows);
                    }
                }
                let stats_for = |values: &Vec<f64>| {
                    let mut v = values.clone();
                    v.sort_by(f64::total_cmp);
                    json!({"median_ms":v[10],"min_ms":v[0],"p95_ms":v[19],"max_ms":v[20],"samples_ms":values})
                };
                let result = json!({"analyzed":stats,"range":range,"lane":lane,"rows":expected.len(),"before":stats_for(&samples[0]),"after":stats_for(&samples[1]),"before_plan":plan(&c,&before,from,to,lane),"after_plan":plan(&c,after,from,to,lane),"equivalent":true});
                println!(
                    "{}",
                    json!({"analyzed":stats,"range":range,"lane":lane,"rows":expected.len(),"before_ms":result["before"]["median_ms"],"after_ms":result["after"]["median_ms"]})
                );
                results.push(result);
            }
        }
    }
    let result = json!({"sqlite_version":rusqlite::version(),"events":180000,"sessions":3600,"lanes":120,"iterations":21,"warmups":3,"limit":200,"results":results});
    fs::write(
        root.join("session-query-measure.json"),
        serde_json::to_string_pretty(&result).unwrap(),
    )
    .unwrap();
}
