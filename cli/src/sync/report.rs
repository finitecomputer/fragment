//! What a pass did: its report, and the exit code that says so.

use serde::Serialize;

#[derive(Default, Serialize, Debug, Clone)]
pub struct Report {
    pub pushed: Vec<String>,
    pub pulled: Vec<String>,
    pub deleted_remote: Vec<String>,
    pub deleted_local: Vec<String>,
    pub withheld_deletions: Vec<String>,
    pub conflicts: Vec<String>,
    /// the token only read (a viewer's): nothing was sent
    pub read_only: bool,
    /// what a writer's mirror would have sent, kept in the folder unsent
    /// (a file deleted here is pulled back): its token only read
    pub unsent: Vec<String>,
    /// synced paths the scan no longer takes, though something is still
    /// there (a `.gitignore`d file, what is not a regular file): kept as
    /// they are, never sent, and the fragment keeps its copy
    pub outside_sync: Vec<String>,
    /// paths in the folder or the repo that are not repo paths
    /// (`valid_repo_path`): never pushed, pulled, or deleted. One in the
    /// repo was written past the cell (a storage token's commit); its
    /// absolute or `..` path would have landed outside the folder.
    pub refused: Vec<String>,
    pub mass_delete_guard: Option<usize>,
    pub scan: ScanStats,
    pub mode: String,
    /// main's head as the pass left it (its own commit, when one landed)
    #[serde(skip)]
    pub head: Option<String>,
    /// a commit of this pass landed (or may have: its answer was lost)
    #[serde(skip)]
    pub landed: bool,
}

#[derive(Default, Serialize, Clone, Copy, Debug)]
pub struct ScanStats {
    pub files: usize,
    pub hashed: usize,
}

impl Report {
    /// exit code: 0 clean, 3 conflicts present, 4 mass-deletion guard, 5
    /// changes not sent (the fragment only read)
    pub fn exit_code(&self) -> i32 {
        if self.mass_delete_guard.is_some() {
            4
        } else if !self.conflicts.is_empty() {
            3
        } else if !self.unsent.is_empty() {
            5
        } else {
            0
        }
    }

    pub fn print(&self) {
        let p = |label: &str, v: &Vec<String>| {
            if !v.is_empty() {
                println!("  {label}: {}", v.len());
                for f in v {
                    println!("    {f}");
                }
            }
        };
        p("pushed", &self.pushed);
        p("pulled", &self.pulled);
        p("deleted remotely", &self.deleted_remote);
        p("deleted locally", &self.deleted_local);
        if !self.withheld_deletions.is_empty() {
            println!("  deletions withheld (pull mode; --prune to apply): {})", self.withheld_deletions.len());
        }
        if !self.refused.is_empty() {
            println!("  REFUSED ({}): not repo paths, so nothing was written, pushed, or deleted for them", self.refused.len());
            for f in &self.refused {
                println!("    {f}");
            }
        }
        if !self.conflicts.is_empty() {
            println!("  CONFLICTS ({}): local keeps yours; the remote copy is saved beside it (.conflict-…)", self.conflicts.len());
            for f in &self.conflicts {
                println!("    {f}");
            }
        }
        if !self.unsent.is_empty() {
            println!("  NOT SENT ({}): you may read this fragment, not change it. Your changes stay here, unsent; a file you deleted was pulled back", self.unsent.len());
            for f in &self.unsent {
                println!("    {f}");
            }
        }
        if !self.outside_sync.is_empty() {
            println!("  NO LONGER SYNCED ({}): ignored here now, or not a file; kept as they are, and the fragment keeps its copy", self.outside_sync.len());
            for f in &self.outside_sync {
                println!("    {f}");
            }
        }
        if let Some(n) = self.mass_delete_guard {
            println!("  REFUSING to propagate {n} deletion(s) — folder looks unmounted/reset.");
            println!("  re-run with --apply-mass-delete if this is intended");
        }
        let clean = self.pushed.is_empty()
            && self.pulled.is_empty()
            && self.conflicts.is_empty()
            && self.deleted_remote.is_empty()
            && self.deleted_local.is_empty()
            && self.unsent.is_empty()
            && self.outside_sync.is_empty();
        if clean {
            println!("all {} files match the repo", self.scan.files);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a pass tells a script what happened by its exit code alone,
    /// as GUIDE.md says, and a tripped guard is the code a script sees even
    /// beside conflicts (it needs someone to look before anything more
    /// moves). Method: a report of each outcome, the codes written out.
    /// The e2e's sync section checks one of them through the process.
    #[test]
    fn a_report_exits_with_its_outcome() {
        let paths = |ps: &[&str]| ps.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let busy = Report {
            pushed: paths(&["a.md"]),
            pulled: paths(&["b.md"]),
            deleted_remote: paths(&["c.md"]),
            deleted_local: paths(&["d.md"]),
            withheld_deletions: paths(&["e.md"]),
            ..Report::default()
        };
        let conflicted = Report { conflicts: paths(&["f.md (remote copy: f.conflict-1-deadbeef.md)"]), ..Report::default() };
        let guarded = Report { mass_delete_guard: Some(15), ..Report::default() };
        let both = Report { conflicts: paths(&["f.md"]), mass_delete_guard: Some(15), ..Report::default() };
        let unsent = Report { unsent: paths(&["g.md"]), read_only: true, ..Report::default() };
        let unsent_and_conflicted = Report { conflicts: paths(&["f.md"]), ..unsent.clone() };
        assert_eq!(Report::default().exit_code(), 0);
        assert_eq!(busy.exit_code(), 0, "work done, withheld deletions too, is a clean pass");
        assert_eq!(conflicted.exit_code(), 3);
        assert_eq!(guarded.exit_code(), 4);
        assert_eq!(both.exit_code(), 4);
        assert_eq!(unsent.exit_code(), 5);
        assert_eq!(unsent_and_conflicted.exit_code(), 3, "a conflict is the code a script sees beside changes unsent");
        let guide = include_str!("../../GUIDE.md");
        assert!(guide.contains("**Exit codes**: 0 clean, 1 failure, 3 conflicts, 4 guard tripped, 5 changes not sent (you may only read it)."), "GUIDE.md names the codes");
    }
}
