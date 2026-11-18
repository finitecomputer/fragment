//! The size a computer's container starts at, from the book's name for it.

use serde::Serialize;

/// The size a computer's container starts at (`ctx.container.start`'s
/// `instance`): one of Containers' named types, or a size of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum InstanceSize {
    Named(String),
    #[serde(rename_all = "camelCase")]
    Custom { vcpu: u32, memory_mib: u32, disk_mb: u32 },
}

/// Containers' named instance types (developers.cloudflare.com/containers/platform/limits).
const NAMED_INSTANCES: [&str; 6] = ["lite", "basic", "standard-1", "standard-2", "standard-3", "standard-4"];

/// The disk of every custom instance: Containers' most for one (its limits
/// page, "Maximum Disk 20 GB"), whatever its memory ("Custom Container
/// instance types no longer have a disk to memory ratio limit", changelog
/// 2026-09-29). The image shares it (the Hermes image is 5.9 GB), and a
/// start restores `/data` onto what is left with 256 MiB to spare (the
/// Sandbox SDK's floor), so a computer whose `/data` filled a 12 GB disk
/// could never start again (2026-11-03). The price still counts 12 GB
/// (`DEFAULT_INSTANCES`; docs/technical-debt-ledger.md).
pub const DISK_MB: u32 = 20_000;

/// The size an instance of the price book names: a named type as it is,
/// or `<n>vcpu-<m>gib` (the book's own names, decision 13's `2vcpu-6gib`)
/// as a custom size with a disk of `DISK_MB`, within Containers' bounds for
/// one: 1 to 4 vCPU, at most 12 GiB, at least 3 GiB a vCPU.
pub fn instance_size(name: &str) -> Result<InstanceSize, String> {
    if NAMED_INSTANCES.contains(&name) {
        return Ok(InstanceSize::Named(name.to_string()));
    }
    let custom = name.strip_suffix("gib").and_then(|n| n.split_once("vcpu-")).and_then(|(v, m)| Some((v.parse::<u32>().ok()?, m.parse::<u32>().ok()?)));
    let Some((vcpu, gib)) = custom else {
        return Err(format!("an instance is one of {} or <n>vcpu-<m>gib, not {name:?}", NAMED_INSTANCES.join(", ")));
    };
    if !(1..=4).contains(&vcpu) || gib > 12 || gib < 3 * vcpu {
        return Err(format!("{name}: a custom instance has 1 to 4 vCPU and at least 3 GiB a vCPU, at most 12 GiB"));
    }
    Ok(InstanceSize::Custom { vcpu, memory_mib: gib * 1024, disk_mb: DISK_MB })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::price::defaults::INSTANCE;

    /// The book's names start the size they price; named types pass as
    /// they are; a size Containers refuses is refused here first.
    #[test]
    fn instance_sizes() {
        assert_eq!(instance_size(INSTANCE), Ok(InstanceSize::Custom { vcpu: 2, memory_mib: 6144, disk_mb: 20_000 }), "Containers' most disk, whatever the memory");
        assert_eq!(serde_json::to_value(instance_size("2vcpu-6gib").unwrap()).unwrap(), serde_json::json!({ "vcpu": 2, "memoryMib": 6144, "diskMb": 20000 }));
        assert_eq!(instance_size("1vcpu-3gib"), Ok(InstanceSize::Custom { vcpu: 1, memory_mib: 3072, disk_mb: 20_000 }));
        assert_eq!(instance_size("standard-3"), Ok(InstanceSize::Named("standard-3".into())));
        assert_eq!(serde_json::to_value(instance_size("standard-3").unwrap()).unwrap(), serde_json::json!("standard-3"));
        assert_eq!(instance_size("4vcpu-12gib"), Ok(InstanceSize::Custom { vcpu: 4, memory_mib: 12_288, disk_mb: 20_000 }));
        for bad in ["", "huge", "2vcpu-4gib", "5vcpu-15gib", "0vcpu-3gib", "1vcpu-13gib", "2vcpu-6", "vcpu-6gib", "-1vcpu-6gib"] {
            assert!(instance_size(bad).is_err(), "{bad:?}");
        }
    }
}
