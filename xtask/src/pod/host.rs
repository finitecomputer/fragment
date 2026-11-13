//! The build host: where mkosi runs. mkosi needs Linux and root, so it is
//! this machine when that is Linux (`"host": "local"`), or a Compute
//! Engine VM reached with `gcloud compute ssh` (`"host": "gce"`), as the
//! proof of concept's build box was. A host's command is written once, as
//! the line its shell runs (`HostCmd::line`), and this machine runs it
//! with `sh -c` or hands it to `gcloud compute ssh --command`: the same
//! line either way, so a dry run prints what runs.

use super::config::HostKind;
use super::plan::{quote, words, Cmd};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub project: String,
    pub kind: HostKind,
}

/// A command on the build host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCmd {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    /// Its output into a file on the host.
    pub out: Option<Redirect>,
}

/// Where a host command's output goes: a file on the host, appended to or
/// replaced, with its errors too (a log) or alone (a file it writes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub path: String,
    pub append: Append,
    pub errors: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Append {
    Yes,
    No,
}

impl HostCmd {
    pub fn new<S: Into<String>>(argv: impl IntoIterator<Item = S>) -> Self {
        Self { argv: argv.into_iter().map(Into::into).collect(), cwd: None, env: Vec::new(), out: None }
    }

    pub fn cwd(self, dir: impl Into<String>) -> Self {
        Self { cwd: Some(dir.into()), ..self }
    }

    pub fn env(mut self, k: &str, v: impl Into<String>) -> Self {
        self.env.push((k.to_string(), v.into()));
        self
    }

    /// Output and errors into `path` on the host.
    pub fn log(self, path: impl Into<String>, append: Append) -> Self {
        Self { out: Some(Redirect { path: path.into(), append, errors: true }), ..self }
    }

    /// Output alone into `path` on the host.
    pub fn to(self, path: impl Into<String>, append: Append) -> Self {
        Self { out: Some(Redirect { path: path.into(), append, errors: false }), ..self }
    }

    /// The line the host's shell runs.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(d) = &self.cwd {
            parts.push(format!("cd {} &&", quote(d)));
        }
        if !self.env.is_empty() {
            let env: Vec<String> = self.env.iter().map(|(k, v)| quote(&format!("{k}={v}"))).collect();
            parts.push(format!("env {}", env.join(" ")));
        }
        parts.push(words(&self.argv));
        if let Some(r) = &self.out {
            let op = if r.append == Append::Yes { ">>" } else { ">" };
            let errors = if r.errors { " 2>&1" } else { "" };
            parts.push(format!("{op} {}{errors}", quote(&r.path)));
        }
        parts.join(" ")
    }
}

impl Host {
    /// The command this machine runs for `c`.
    pub fn cmd(&self, c: &HostCmd) -> Cmd {
        let line = c.line();
        match &self.kind {
            HostKind::Local => Cmd::new(["sh".to_string(), "-c".to_string(), line]),
            HostKind::Gce { instance, zone } => Cmd::new(
                gcloud(&self.project, &["compute", "ssh", instance, &format!("--zone={zone}"), &format!("--command={line}")]).argv,
            ),
        }
    }
}

/// `gcloud` for this project, never prompting: every answer a prompt would
/// take is a flag the plan shows.
pub fn gcloud(project: &str, args: &[&str]) -> Cmd {
    let mut argv = vec!["gcloud".to_string(), format!("--project={project}"), "--quiet".to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    Cmd::new(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_runs_here_or_over_ssh() {
        let c = HostCmd::new(["sudo", "/srv/b/mkosi/bin/mkosi", "build"]).cwd("/srv/b/work").env("A", "x y").log("/srv/b/build.log", Append::No);
        assert_eq!(c.line(), "cd /srv/b/work && env 'A=x y' sudo /srv/b/mkosi/bin/mkosi build > /srv/b/build.log 2>&1");
        let local = Host { project: "pod-project".into(), kind: HostKind::Local };
        assert_eq!(local.cmd(&c).argv, ["sh", "-c", &c.line()]);
        assert_eq!(HostCmd::new(["sha256sum", "pod.raw"]).to("sums", Append::Yes).line(), "sha256sum pod.raw >> sums");
        let gce = Host { project: "pod-project".into(), kind: HostKind::Gce { instance: "pod-build".into(), zone: "us-central1-f".into() } };
        assert_eq!(
            gce.cmd(&c).shown(),
            format!("gcloud --project=pod-project --quiet compute ssh pod-build --zone=us-central1-f {}", quote(&format!("--command={}", c.line())))
        );
    }
}
