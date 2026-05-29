//! Timeline density and cross-repo correlations (Phase 3).
//!
//! Historical per-lane activity isn't tracked, so the timeline is per-repo: density is the
//! commit count per time bucket, and correlations are the Jaccard similarity of two repos'
//! active-bucket sets.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::model::{Commit, Correlation, RepoId, TimelineData, TimelineRow};

/// Density block characters, low to high.
pub const DENSITY_CHARS: [&str; 6] = [" ", "▁", "░", "▒", "▓", "█"];

/// Map a per-bucket count to a density level (0–5).
pub fn density_level(count: u32) -> u8 {
    match count {
        0 => 0,
        1 => 1,
        2..=3 => 2,
        4..=7 => 3,
        8..=15 => 4,
        _ => 5,
    }
}

/// The density character for a level.
pub fn density_char(level: u8) -> &'static str {
    DENSITY_CHARS[(level as usize).min(5)]
}

/// Build the timeline: per-repo density rows + Jaccard correlations over `[from, to)`.
pub fn build_timeline(
    commits: &[Commit],
    repo_names: &HashMap<RepoId, String>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    bucket_secs: i64,
) -> TimelineData {
    let span = (to - from).num_seconds().max(0);
    let bucket = bucket_secs.max(1);
    // Number of buckets covering the half-open range [from, to) (ceil division).
    let n_buckets = (((span + bucket - 1) / bucket).max(1)) as usize;

    let mut counts: BTreeMap<RepoId, Vec<u32>> = BTreeMap::new();
    for c in commits {
        if c.time < from || c.time >= to {
            continue;
        }
        let idx = ((c.time - from).num_seconds() / bucket_secs.max(1)) as usize;
        if idx < n_buckets {
            counts
                .entry(c.repo_id)
                .or_insert_with(|| vec![0; n_buckets])[idx] += 1;
        }
    }

    let mut rows = Vec::new();
    let mut active: Vec<(RepoId, HashSet<usize>)> = Vec::new();
    for (repo_id, c) in &counts {
        let density: Vec<u8> = c.iter().map(|&n| density_level(n)).collect();
        let set: HashSet<usize> = c
            .iter()
            .enumerate()
            .filter(|(_, &n)| n > 0)
            .map(|(i, _)| i)
            .collect();
        active.push((*repo_id, set));
        rows.push(TimelineRow {
            repo_id: *repo_id,
            repo_name: repo_names.get(repo_id).cloned().unwrap_or_default(),
            density,
        });
    }

    let correlations = correlations(&active, repo_names);
    TimelineData {
        from,
        to,
        bucket_secs,
        rows,
        correlations,
    }
}

/// Jaccard similarity of active-bucket sets for every repo pair, > 0.1, sorted desc.
fn correlations(
    active: &[(RepoId, HashSet<usize>)],
    names: &HashMap<RepoId, String>,
) -> Vec<Correlation> {
    let mut out = Vec::new();
    for i in 0..active.len() {
        for j in (i + 1)..active.len() {
            let (a, sa) = &active[i];
            let (b, sb) = &active[j];
            let inter = sa.intersection(sb).count();
            let uni = sa.union(sb).count();
            if uni == 0 {
                continue;
            }
            let jaccard = inter as f64 / uni as f64;
            if jaccard > 0.1 {
                out.push(Correlation {
                    a: names.get(a).cloned().unwrap_or_default(),
                    b: names.get(b).cloned().unwrap_or_default(),
                    windows: inter as u32,
                    overlap: (jaccard * 100.0).round() / 100.0,
                });
            }
        }
    }
    out.sort_by(|x, y| {
        y.overlap
            .partial_cmp(&x.overlap)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn commit(repo_id: RepoId, secs: i64, base: DateTime<Utc>) -> Commit {
        Commit {
            oid: format!("{:040x}", secs).parse().unwrap(),
            repo_id,
            author_name: "t".into(),
            author_email: "t@e".into(),
            summary: "c".into(),
            time: base + chrono::Duration::seconds(secs),
            parent_count: 1,
        }
    }

    #[test]
    fn density_levels() {
        assert_eq!(density_level(0), 0);
        assert_eq!(density_level(1), 1);
        assert_eq!(density_level(3), 2);
        assert_eq!(density_level(7), 3);
        assert_eq!(density_level(15), 4);
        assert_eq!(density_level(99), 5);
    }

    #[test]
    fn timeline_buckets_and_correlation() {
        let base = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let from = base;
        let to = base + chrono::Duration::seconds(300);
        let bucket = 100; // 3 buckets

        // repo 1 active in buckets 0 and 2; repo 2 active in buckets 0 and 2 (full overlap).
        let commits = vec![
            commit(1, 10, base),
            commit(1, 210, base),
            commit(2, 20, base),
            commit(2, 220, base),
        ];
        let mut names = HashMap::new();
        names.insert(1, "a".to_string());
        names.insert(2, "b".to_string());

        let t = build_timeline(&commits, &names, from, to, bucket);
        assert_eq!(t.rows.len(), 2);
        // Each repo: bucket 0 and 2 have 1 commit (level 1), bucket 1 has 0.
        assert_eq!(t.rows[0].density, vec![1, 0, 1]);
        assert_eq!(t.correlations.len(), 1);
        assert!(
            (t.correlations[0].overlap - 1.0).abs() < 1e-9,
            "full overlap"
        );
        assert_eq!(t.correlations[0].windows, 2);
    }
}
