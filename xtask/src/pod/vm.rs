//! `cargo xtask pod vm create|start|stop|describe|delete`: a pod VM from
//! a published image (`pod image publish`), with its data disk, on
//! Compute Engine. It replaces the proof of concept's boot.sh and the VM
//! half of box.sh.
//!
//! These spend money and delete things, so each subcommand that changes
//! anything runs only with its own confirming flag (`create` and `start`:
//! `--confirm-spend`; `stop`: `--confirm-stop`; `delete`:
//! `--confirm-delete`), and without it prints its plan and refuses.
//! `--dry-run` prints the plan (every gcloud call, in order) and runs
//! nothing. A delete says what it deletes and what it keeps before it
//! runs; the data disk (the owner's volume) is deleted only when it is
//! named again with `--delete-data-disk <name>`.

use std::path::Path;

use anyhow::Result;

use super::config::{Config, Vm};
use super::host::gcloud;
use super::plan::{Otherwise, Plan, Probe, Step, Stdout};

/// The data disk's device name in the guest: pod-init's config names
/// `/dev/disk/by-id/google-pod-data` (pod-image/…/pod-init.json), which
/// the image's udev rule makes from it.
pub const DATA_DEVICE: &str = "pod-data";

struct Ctx<'a> {
    cfg: &'a Config,
    vm: &'a Vm,
    image: &'a str,
}

impl Ctx<'_> {
    fn g(&self, args: &[&str]) -> super::plan::Cmd {
        gcloud(&self.cfg.project, args)
    }

    fn zone(&self) -> String {
        format!("--zone={}", self.vm.zone)
    }

    fn listed(&self, kind: &str, name: &str) -> Probe {
        let (zones, filter) = (format!("--zones={}", self.vm.zone), format!("--filter=name={name}"));
        Probe::Lists(self.g(&["compute", kind, "list", &zones, &filter, "--format=value(name)"]), name.to_string())
    }

    fn describe_line(&self) -> Step {
        Step::new(format!("{} as it is", self.vm.name), self.g(&["compute", "instances", "describe", &self.vm.name, &self.zone(), "--format=value(name,status,networkInterfaces[0].accessConfigs[0].natIP)"]))
    }
}

/// `vm create`: the data disk if it is not there, then the VM, which
/// boots the image locked.
fn create(c: &Ctx<'_>) -> Plan {
    let v = c.vm;
    let mut p = Plan::new(format!("cargo xtask pod vm create: {} ({}, {}) from image {} in {}", v.name, v.machine_type, v.variant.name(), c.image, v.zone));
    p.note(format!("keeps data disk {} if it exists (the owner's volume, whatever it holds), else makes it blank ({} GB)", v.data_disk.name, v.data_disk.size_gb));
    if let Some(d) = &v.max_run_duration {
        p.note(format!("the VM deletes itself after {d} (max_run_duration); its data disk stays"));
    }
    p.push(Step::gate(format!("no VM {} yet", v.name), c.listed("instances", &v.name), false, Otherwise::Refuse(format!("VM {} already exists in {}: `pod vm describe` it, or `pod vm delete` it first", v.name, v.zone))));
    let d = &v.data_disk;
    let data_labels = c.cfg.labels_with(&[("pod-role", "data")]);
    p.push(
        Step::new(
            format!("data disk {}", d.name),
            c.g(&["compute", "disks", "create", &d.name, &c.zone(), &format!("--size={}GB", d.size_gb), &format!("--type={}", d.kind), &format!("--labels={data_labels}")]),
        )
        .unless(c.listed("disks", &d.name), Otherwise::Skip(format!("{} exists: kept as it is", d.name))),
    );
    let labels = c.cfg.labels_with(&[("pod-role", "vm"), ("pod-variant", v.variant.name())]);
    let mut args: Vec<String> = [
        "compute", "instances", "create", &v.name,
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([
        c.zone(),
        format!("--machine-type={}", v.machine_type),
        format!("--image={}", c.image),
        format!("--boot-disk-size={}GB", v.boot_disk_gb),
        format!("--boot-disk-type={}", v.boot_disk_type),
        format!("--disk=name={},device-name={DATA_DEVICE},mode=rw,boot=no,auto-delete=no", d.name),
        "--confidential-compute-type=TDX".into(),
        "--maintenance-policy=TERMINATE".into(),
    ])
    .collect();
    if let Some(run) = &v.max_run_duration {
        args.extend([format!("--max-run-duration={run}"), "--instance-termination-action=DELETE".into()]);
    }
    args.push(format!("--network={}", v.network));
    if let Some(s) = &v.subnet {
        args.push(format!("--subnet={s}"));
    }
    if !v.tags.is_empty() {
        args.push(format!("--tags={}", v.tags.join(",")));
    }
    args.extend(["--no-service-account".into(), "--no-scopes".into(), format!("--labels={labels}")]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    p.push(Step::new(format!("VM {}: it boots locked, pod-init on port 7000", v.name), c.g(&refs)));
    let boot_labels = c.cfg.labels_with(&[("pod-role", "boot")]);
    p.push(Step::new(format!("label its boot disk {}", v.name), c.g(&["compute", "disks", "add-labels", &v.name, &c.zone(), &format!("--labels={boot_labels}")]).stdout(Stdout::Discard)));
    p.push(c.describe_line());
    p
}

fn start(c: &Ctx<'_>) -> Plan {
    let mut p = Plan::new(format!("cargo xtask pod vm start: {} in {}", c.vm.name, c.vm.zone));
    p.note("it boots locked, with a new boot key: the owner unlocks it again");
    p.push(Step::new(format!("start {}", c.vm.name), c.g(&["compute", "instances", "start", &c.vm.name, &c.zone()])));
    p.push(c.describe_line());
    p
}

fn stop(c: &Ctx<'_>) -> Plan {
    let mut p = Plan::new(format!("cargo xtask pod vm stop: {} in {}", c.vm.name, c.vm.zone));
    p.note("the volume closes with the VM; its disks stay, and still cost money");
    p.push(Step::new(format!("stop {}", c.vm.name), c.g(&["compute", "instances", "stop", &c.vm.name, &c.zone()])));
    p.push(c.describe_line());
    p
}

/// `vm describe`: the VM and its data disk; with `--save <dir>`, the
/// instance as JSON and its serial console into that folder.
fn describe(c: &Ctx<'_>, save: Option<&Path>) -> Plan {
    let v = c.vm;
    let mut p = Plan::new(format!("cargo xtask pod vm describe: {} in {}", v.name, v.zone));
    p.push(c.describe_line());
    p.push(Step::new(format!("data disk {}", v.data_disk.name), c.g(&["compute", "disks", "describe", &v.data_disk.name, &c.zone(), "--format=value(name,status,sizeGb,users.basename())"])));
    if let Some(dir) = save {
        p.push(Step::new("the instance, as JSON", c.g(&["compute", "instances", "describe", &v.name, &c.zone(), "--format=json"]).stdout(Stdout::File(dir.join("instance.json")))));
        p.push(Step::new("its serial console", c.g(&["compute", "instances", "get-serial-port-output", &v.name, &c.zone()]).stdout(Stdout::File(dir.join("serial.txt")))));
    }
    p
}

/// `vm delete`: the VM and its boot disk; the data disk too only when
/// named.
fn delete(c: &Ctx<'_>, data_disk: bool) -> Plan {
    let v = c.vm;
    let mut p = Plan::new(format!("cargo xtask pod vm delete: {} in {}", v.name, v.zone));
    p.note(format!("deletes: VM {} and its boot disk {} (made with it, deleted with it)", v.name, v.name));
    if data_disk {
        p.note(format!("deletes: data disk {}, the owner's volume: what it holds is lost", v.data_disk.name));
    } else {
        p.note(format!("keeps: data disk {} (the owner's volume; --delete-data-disk {} deletes it too)", v.data_disk.name, v.data_disk.name));
    }
    let gone = Otherwise::Skip(format!("no VM {}", v.name));
    p.push(
        Step::new(format!("what {} has attached", v.name), c.g(&["compute", "instances", "describe", &v.name, &c.zone(), "--format=value(name,status,disks[].source.basename().list())"]))
            .only_if(c.listed("instances", &v.name), gone.clone()),
    );
    p.push(Step::new(format!("delete VM {}", v.name), c.g(&["compute", "instances", "delete", &v.name, &c.zone()])).only_if(c.listed("instances", &v.name), gone));
    if data_disk {
        p.push(
            Step::new(format!("delete data disk {}", v.data_disk.name), c.g(&["compute", "disks", "delete", &v.data_disk.name, &c.zone()]))
                .only_if(c.listed("disks", &v.data_disk.name), Otherwise::Skip(format!("no disk {}", v.data_disk.name))),
        );
    }
    p
}

/// What `pod vm` was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sub {
    Create,
    Start,
    Stop,
    Describe { save: Option<std::path::PathBuf> },
    Delete { data_disk: Option<String> },
}

impl Sub {
    /// The flag that lets it run; describe needs none.
    pub fn confirm_flag(&self) -> Option<&'static str> {
        match self {
            Self::Create | Self::Start => Some("--confirm-spend"),
            Self::Stop => Some("--confirm-stop"),
            Self::Delete { .. } => Some("--confirm-delete"),
            Self::Describe { .. } => None,
        }
    }
}

pub fn plan(cfg: &Config, sub: &Sub) -> Result<Plan> {
    let vm = cfg.vm()?;
    let image = cfg.images()?.of(vm.variant);
    let c = Ctx { cfg, vm, image };
    Ok(match sub {
        Sub::Create => create(&c),
        Sub::Start => start(&c),
        Sub::Stop => stop(&c),
        Sub::Describe { save } => describe(&c, save.as_deref()),
        Sub::Delete { data_disk } => {
            if let Some(named) = data_disk {
                anyhow::ensure!(named == &vm.data_disk.name, "--delete-data-disk names {named:?}, but this VM's data disk is {:?}", vm.data_disk.name);
            }
            delete(&c, data_disk.is_some())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::config::tests::example;
    use super::super::plan::stub::Stub;
    use super::super::plan::Ran;
    use super::*;

    const G: &str = "gcloud --project=pod-project --quiet compute";

    fn lines(p: &Plan) -> Vec<String> {
        p.render().lines().filter_map(|l| l.trim().strip_prefix("$ ").map(str::to_string)).collect()
    }

    /// create's commands, against boot.sh's: its instances create line
    /// flag for flag, but that the data disk is made when missing (the
    /// script took an existing one, POD_IMG_DATA_DISK), the boot disk's
    /// size is the config's (the script asked the image's), the port-7000
    /// firewall rule is the network's own (the script made one for the
    /// laptop's address, which the P runs skipped), and there is no N2D
    /// (SEV-SNP) path: a pod runs on TDX.
    #[test]
    fn create_runs_what_boot_sh_ran() {
        let cfg = example();
        let p = plan(&cfg, &Sub::Create).unwrap();
        let l = lines(&p);
        assert_eq!(l[0], format!("{G} disks create pod-1-data --zone=us-central1-f --size=20GB --type=pd-balanced --labels=owner=alex,pod-role=data"));
        assert_eq!(
            l[1],
            format!(
                "{G} instances create pod-1 --zone=us-central1-f --machine-type=c3-standard-8 --image=pod-p --boot-disk-size=10GB --boot-disk-type=pd-balanced \
--disk=name=pod-1-data,device-name=pod-data,mode=rw,boot=no,auto-delete=no --confidential-compute-type=TDX --maintenance-policy=TERMINATE \
--max-run-duration=3h --instance-termination-action=DELETE --network=pod-net --subnet=pod-subnet --tags=pod-7000 --no-service-account --no-scopes \
--labels=owner=alex,pod-role=vm,pod-variant=locked"
            )
        );
        assert_eq!(l[2], format!("{G} disks add-labels pod-1 --zone=us-central1-f --labels=owner=alex,pod-role=boot > /dev/null"));
        assert_eq!(l[3], format!("{G} instances describe pod-1 --zone=us-central1-f '--format=value(name,status,networkInterfaces[0].accessConfigs[0].natIP)'"));
        assert!(p.render().contains(&format!("first: {G} instances list --zones=us-central1-f --filter=name=pod-1 '--format=value(name)'\n   if it lists pod-1: refuse")));
        let mut debug = example();
        debug.vm.as_mut().unwrap().variant = super::super::Variant::Debug;
        debug.vm.as_mut().unwrap().max_run_duration = None;
        let l = lines(&plan(&debug, &Sub::Create).unwrap());
        assert!(l[1].contains("--image=pod-p-debug") && l[1].contains("pod-variant=debug") && !l[1].contains("max-run-duration"), "{}", l[1]);
    }

    /// Replay: a second create of an existing VM is refused before
    /// anything is made; a data disk that exists is kept, not made again.
    #[test]
    fn a_second_create_is_refused_and_a_data_disk_kept() {
        let cfg = example();
        let p = plan(&cfg, &Sub::Create).unwrap();
        let mut s = Stub::default().answer("instances list", "pod-1\n", true);
        let e = p.run(&mut s).unwrap_err();
        assert!(e.to_string().contains("already exists"), "{e}");
        assert_eq!(s.ran.len(), 1, "{:?}", s.ran);
        let mut s = Stub::default().answer("disks list", "pod-1-data\n", true);
        assert_eq!(p.run(&mut s).unwrap(), Ran::All);
        assert!(!s.ran.iter().any(|c| c.contains("disks create")) && s.ran.iter().any(|c| c.contains("instances create pod-1")), "{:?}", s.ran);
    }

    /// delete names what goes and what stays; the data disk goes only
    /// when named again, and only by its own name. Replay: a VM already
    /// gone is skipped, not an error.
    #[test]
    fn delete_says_what_it_deletes_and_keeps_the_volume() {
        let cfg = example();
        let p = plan(&cfg, &Sub::Delete { data_disk: None }).unwrap();
        let text = p.render();
        assert!(text.contains("deletes: VM pod-1 and its boot disk pod-1") && text.contains("keeps: data disk pod-1-data"), "{text}");
        assert_eq!(lines(&p), [format!("{G} instances describe pod-1 --zone=us-central1-f '--format=value(name,status,disks[].source.basename().list())'"), format!("{G} instances delete pod-1 --zone=us-central1-f")]);
        let both = plan(&cfg, &Sub::Delete { data_disk: Some("pod-1-data".into()) }).unwrap();
        assert!(both.render().contains("deletes: data disk pod-1-data, the owner's volume"));
        assert_eq!(lines(&both).last().unwrap(), &format!("{G} disks delete pod-1-data --zone=us-central1-f"));
        assert!(plan(&cfg, &Sub::Delete { data_disk: Some("pod-2-data".into()) }).is_err(), "only this VM's data disk, by its name");
        let mut s = Stub::default();
        assert_eq!(p.run(&mut s).unwrap(), Ran::All);
        assert!(!s.ran.iter().any(|c| c.contains("instances delete")), "nothing to delete: {:?}", s.ran);
        let mut s = Stub::default().answer("instances list", "pod-1\n", true);
        p.run(&mut s).unwrap();
        assert!(s.ran.iter().any(|c| c.contains("instances delete pod-1")) && !s.ran.iter().any(|c| c.contains("disks delete")));
    }

    #[test]
    fn start_stop_and_describe() {
        let cfg = example();
        assert_eq!(lines(&plan(&cfg, &Sub::Start).unwrap())[0], format!("{G} instances start pod-1 --zone=us-central1-f"));
        assert_eq!(lines(&plan(&cfg, &Sub::Stop).unwrap())[0], format!("{G} instances stop pod-1 --zone=us-central1-f"));
        let l = lines(&plan(&cfg, &Sub::Describe { save: Some("/tmp/r".into()) }).unwrap());
        assert_eq!(l[3], format!("{G} instances get-serial-port-output pod-1 --zone=us-central1-f > /tmp/r/serial.txt"));
        assert_eq!(Sub::Describe { save: None }.confirm_flag(), None);
        assert_eq!(Sub::Delete { data_disk: None }.confirm_flag(), Some("--confirm-delete"));
    }
}
