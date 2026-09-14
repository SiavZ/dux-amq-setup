//! The recency ordering of the project lists, core-owned and mirrored by hand in
//! the web's `crates/dux-web/web/src/lib/projectOrder.ts`. The rule lives here
//! and the mirror is kept in step by hand: both test files carry the same cases,
//! and nothing links the two suites, so a change to the rule is made in both
//! places.
//!
//! Both screens that list every project (the web's New-agent picker and the
//! TUI's project chooser) ask the same question: what did the user touch most
//! recently? A project's instant is the later of when it was added and when its
//! newest agent was created; a standalone agent belongs to no project and counts
//! for none.

use chrono::{DateTime, Utc};

use crate::model::{AgentSession, Project};

/// Order the projects newest-touched first, returning indices into `projects`.
///
/// A project with no instant at all (no stored `created_at` and no agents) has
/// nothing to rank on, so it goes last in incoming order. The sort is stable, so
/// ties keep the stored order.
pub fn order_projects_by_recency(projects: &[Project], sessions: &[AgentSession]) -> Vec<usize> {
    let instants: Vec<Option<DateTime<Utc>>> = projects
        .iter()
        .map(|project| last_touched(project, sessions))
        .collect();
    let mut order: Vec<usize> = (0..projects.len()).collect();
    // `None` sorts below every `Some`, so reversing the comparison puts the
    // instant-less projects last and the newest instant first.
    order.sort_by(|a, b| instants[*b].cmp(&instants[*a]));
    order
}

/// The later of when the project was added and when its newest agent was
/// created, or `None` when neither exists.
fn last_touched(project: &Project, sessions: &[AgentSession]) -> Option<DateTime<Utc>> {
    let newest_agent = sessions
        .iter()
        .filter(|session| session.project_id() == Some(project.id.as_str()))
        .map(|session| session.created_at)
        .max();
    match (project.created_at, newest_agent) {
        (Some(added), Some(agent)) => Some(added.max(agent)),
        (Some(added), None) => Some(added),
        (None, Some(agent)) => Some(agent),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_project, sample_session, sample_standalone_session};
    use chrono::TimeZone;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, day, hour, 0, 0).unwrap()
    }

    fn project(id: &str, created_at: Option<DateTime<Utc>>) -> Project {
        let mut p = sample_project(id, &format!("/tmp/{id}"));
        p.created_at = created_at;
        p
    }

    fn agent(id: &str, project_id: &str, created_at: DateTime<Utc>) -> AgentSession {
        let mut s = sample_session(id, project_id, "feature");
        s.created_at = created_at;
        s
    }

    // ── SHARED VECTORS with projectOrder.test.ts ──────────────────────────────

    #[test]
    fn the_newest_agent_carries_its_project_to_the_top() {
        let projects = vec![
            project("old", Some(at(1, 9))),
            project("new", Some(at(2, 9))),
        ];
        let sessions = vec![agent("a1", "old", at(5, 9))];
        assert_eq!(order_projects_by_recency(&projects, &sessions), vec![0, 1]);
    }

    #[test]
    fn a_project_added_after_its_agents_ranks_by_the_added_date() {
        let projects = vec![
            project("added-late", Some(at(9, 9))),
            project("other", Some(at(2, 9))),
        ];
        let sessions = vec![agent("a1", "added-late", at(3, 9))];
        assert_eq!(order_projects_by_recency(&projects, &sessions), vec![0, 1]);
    }

    #[test]
    fn an_empty_project_ranks_by_the_date_it_was_added() {
        let projects = vec![
            project("with-agent", Some(at(1, 9))),
            project("empty", Some(at(4, 9))),
        ];
        let sessions = vec![agent("a1", "with-agent", at(2, 9))];
        assert_eq!(order_projects_by_recency(&projects, &sessions), vec![1, 0]);
    }

    #[test]
    fn a_standalone_agent_lifts_no_project() {
        let projects = vec![
            project("first", Some(at(2, 9))),
            project("second", Some(at(1, 9))),
        ];
        let mut standalone = sample_standalone_session("s1", "/home/someone/work");
        standalone.created_at = at(9, 9);
        let sessions = vec![standalone];
        assert_eq!(order_projects_by_recency(&projects, &sessions), vec![0, 1]);
    }

    #[test]
    fn a_project_with_no_instant_at_all_goes_last() {
        let projects = vec![
            project("unstored", None),
            project("dated", Some(at(1, 9))),
            project("also-unstored", None),
        ];
        assert_eq!(order_projects_by_recency(&projects, &[]), vec![1, 0, 2]);
    }

    #[test]
    fn an_unstored_project_with_an_agent_is_ranked_by_that_agent() {
        let projects = vec![project("dated", Some(at(3, 9))), project("unstored", None)];
        let sessions = vec![agent("a1", "unstored", at(6, 9))];
        assert_eq!(order_projects_by_recency(&projects, &sessions), vec![1, 0]);
    }

    #[test]
    fn ties_keep_the_incoming_order() {
        let projects = vec![
            project("one", Some(at(3, 9))),
            project("two", Some(at(3, 9))),
            project("three", Some(at(3, 9))),
        ];
        assert_eq!(order_projects_by_recency(&projects, &[]), vec![0, 1, 2]);
    }
}
