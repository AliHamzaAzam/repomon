//! Work-session detection (Phase 3).
//!
//! Events here are commits (agent tool-calls and dirty-state changes aren't retained
//! historically). Per repo, contiguous commits with no gap over 30 min form an interval;
//! intervals that overlap in time across repos cluster into one session — `Parallel` if it
//! spans multiple repos, else `Focused`. Sessions shorter than 10 minutes are dropped as
//! noise.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::model::{Commit, RepoId, SessionKind, WorkSession};

const GAP_MINUTES: i64 = 30;
const MIN_SESSION_MINUTES: i64 = 10;

struct Interval {
    repo_id: RepoId,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    count: u32,
}

/// A growing cluster of overlapping intervals (a candidate session).
struct Cluster {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    repos: Vec<RepoId>,
    count: u32,
}

/// Detect work sessions from a set of commits, newest first.
pub fn detect(commits: &[Commit], repo_names: &HashMap<RepoId, String>) -> Vec<WorkSession> {
    // Per-repo contiguous intervals (gap <= 30 min).
    let mut by_repo: HashMap<RepoId, Vec<DateTime<Utc>>> = HashMap::new();
    for c in commits {
        by_repo.entry(c.repo_id).or_default().push(c.time);
    }
    let mut intervals: Vec<Interval> = Vec::new();
    for (repo_id, mut times) in by_repo {
        times.sort();
        let mut from = times[0];
        let mut last = times[0];
        let mut count = 0u32;
        for &t in &times {
            if (t - last).num_minutes() > GAP_MINUTES {
                intervals.push(Interval {
                    repo_id,
                    from,
                    to: last,
                    count,
                });
                from = t;
                count = 0;
            }
            last = t;
            count += 1;
        }
        intervals.push(Interval {
            repo_id,
            from,
            to: last,
            count,
        });
    }

    // Cluster intervals that overlap in time into sessions.
    intervals.sort_by_key(|i| i.from);
    let mut sessions: Vec<WorkSession> = Vec::new();
    let mut cur: Option<Cluster> = None;
    for iv in intervals {
        match &mut cur {
            Some(c) if iv.from <= c.to => {
                if iv.to > c.to {
                    c.to = iv.to;
                }
                if !c.repos.contains(&iv.repo_id) {
                    c.repos.push(iv.repo_id);
                }
                c.count += iv.count;
            }
            _ => {
                if let Some(c) = cur.take() {
                    push_session(&mut sessions, c, repo_names);
                }
                cur = Some(Cluster {
                    from: iv.from,
                    to: iv.to,
                    repos: vec![iv.repo_id],
                    count: iv.count,
                });
            }
        }
    }
    if let Some(c) = cur.take() {
        push_session(&mut sessions, c, repo_names);
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.from));
    sessions
}

fn push_session(out: &mut Vec<WorkSession>, c: Cluster, repo_names: &HashMap<RepoId, String>) {
    if (c.to - c.from).num_minutes() < MIN_SESSION_MINUTES {
        return; // noise
    }
    let kind = if c.repos.len() > 1 {
        SessionKind::Parallel
    } else {
        SessionKind::Focused
    };
    let names = c
        .repos
        .iter()
        .map(|id| repo_names.get(id).cloned().unwrap_or_default())
        .collect();
    out.push(WorkSession {
        from: c.from,
        to: c.to,
        kind,
        repo_ids: c.repos,
        repo_names: names,
        commit_count: c.count,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn commit(repo_id: RepoId, minutes: i64, base: DateTime<Utc>) -> Commit {
        Commit {
            oid: format!("{:040x}", minutes.unsigned_abs()).parse().unwrap(),
            repo_id,
            author_name: "t".into(),
            author_email: "t@e".into(),
            summary: "c".into(),
            time: base + Duration::minutes(minutes),
            parent_count: 1,
        }
    }

    #[test]
    fn splits_on_gap_and_drops_short_sessions() {
        let base = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        // One session 0..20 min (3 commits), then a >30min gap, then a 1-commit blip (dropped).
        let commits = vec![
            commit(1, 0, base),
            commit(1, 10, base),
            commit(1, 20, base),
            commit(1, 80, base), // isolated -> <10min span -> dropped
        ];
        let names = HashMap::from([(1, "a".to_string())]);
        let sessions = detect(&commits, &names);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].kind, SessionKind::Focused);
        assert_eq!(sessions[0].duration_minutes(), 20);
        assert_eq!(sessions[0].commit_count, 3);
    }

    #[test]
    fn overlapping_repos_form_a_parallel_session() {
        let base = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let commits = vec![
            commit(1, 0, base),
            commit(1, 15, base),
            commit(2, 5, base),
            commit(2, 18, base),
        ];
        let names = HashMap::from([(1, "a".to_string()), (2, "b".to_string())]);
        let sessions = detect(&commits, &names);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].kind, SessionKind::Parallel);
        assert_eq!(sessions[0].repo_ids.len(), 2);
    }
}
