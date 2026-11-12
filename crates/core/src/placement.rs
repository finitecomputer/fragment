//! Placement: which sandcastle node a computer runs on (docs/self-host.md,
//! seam 2). A deployment that runs its computers on nodes, not in the
//! runtime's own containers, lists them in `FRAGMENT_NODES`, a JSON object:
//!
//! ```json
//! { "nodes": [ { "id": "box", "url": "http://127.0.0.1:9400", "arch": "x86_64" } ],
//!   "images": {
//!     "stub":   "docker.io/library/fragment-stub@sha256:<64 hex>",
//!     "hermes": { "x86_64": "registry.lan/fragment-hermes@sha256:<64 hex>" } } }
//! ```
//!
//! - **A node**: its id, its API's origin (sandcastle's `sandcastle-node`),
//!   and the architecture it runs. Its secret is the store binding
//!   `secret_binding(id)` (cell/secrets.mjs stands in for one where the
//!   runtime has no Secrets Store), never in this variable.
//! - **An image**, by the name computers are pinned to: one reference for
//!   every architecture, or one per architecture. Each is pinned by its
//!   digest: a computer's version is its reference, and a snapshot is
//!   reused only under the reference it was taken of, so a tag built again
//!   under the same name would start the old image's snapshot and never
//!   tell its owner an update is ready.
//!
//! A computer is placed at its first start and stays on that node for its
//! life: its snapshots are the node's. The rule (`choose`): of the nodes that
//! answer their health, report the architecture they are listed with and
//! have an image for the computer's name, the one with the most room among
//! its VMs (its engine's `vms_max` less its `vms`); a tie goes to the node
//! listed first, the deployment's preference. Room is the engine's own
//! count of what runs now, so it balances awake computers, not every
//! computer placed: a node whose computers all wake at once refuses past
//! its `vms_max`, and that start fails, saying so. Counting placed
//! computers needs one count shared by every computer (the old lane's
//! `Node` object), which is not built.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// The most nodes a deployment lists: every one is probed at once.
pub const NODES_MAX: usize = 16;
/// The most images it names.
pub const IMAGES_MAX: usize = 16;
/// A node's id, at most: the binding of its secret is named after it.
pub const NODE_ID_BYTES_MAX: usize = 32;
/// A node's URL, at most.
pub const URL_BYTES_MAX: usize = 256;
/// An image's name, at most.
pub const IMAGE_NAME_BYTES_MAX: usize = 64;
/// An image's reference, at most (its digest included).
pub const REFERENCE_BYTES_MAX: usize = 512;
/// A node's secret, at least (sandcastle's `auth::Secret`).
pub const SECRET_BYTES_MIN: usize = 32;
/// The binding of a node's secret is this and its id, upper-cased.
pub const SECRET_BINDING_PREFIX: &str = "FRAGMENT_NODE_SECRET_";

/// A node's architecture, as Rust (`std::env::consts::ARCH`) and the
/// node's health name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Arch {
    #[serde(rename = "x86_64")]
    X86_64,
    #[serde(rename = "aarch64")]
    Aarch64,
}

impl Arch {
    pub const ALL: [Arch; 2] = [Arch::X86_64, Arch::Aarch64];

    pub fn name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }

    pub fn parse(s: &str) -> Option<Arch> {
        Arch::ALL.into_iter().find(|a| a.name() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    /// Its API's origin, with no trailing slash.
    pub url: String,
    pub arch: Arch,
}

/// A deployment's nodes and images, checked. Plain lists, in the order the
/// list names them (images by name): a few of each, read by a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nodes {
    nodes: Vec<Node>,
    /// Each image's reference by architecture.
    images: Vec<(String, Vec<(Arch, String)>)>,
}

/// Why `FRAGMENT_NODES` is refused: the cell refuses every computer's
/// start with this, saying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodesError {
    /// Not `{nodes, images}` as above.
    Shape(String),
    /// No nodes, or more than `NODES_MAX`.
    Count(usize),
    /// A node's id is not 1 to 32 of a-z, 0-9 and `-`.
    Id(String),
    /// Two nodes have one id.
    Duplicate(String),
    /// A node's URL is no http(s) origin.
    Url { node: String, why: String },
    /// No images, more than `IMAGES_MAX`, a bad name, or a bad reference.
    Image { image: String, why: String },
}

impl fmt::Display for NodesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodesError::Shape(e) => write!(f, "FRAGMENT_NODES is {{\"nodes\": [{{id, url, arch}}], \"images\": {{name: reference | {{arch: reference}}}}}}: {e}"),
            NodesError::Count(n) => write!(f, "FRAGMENT_NODES lists 1 to {NODES_MAX} nodes, not {n}"),
            NodesError::Id(id) => write!(f, "FRAGMENT_NODES: {id:?} is not a node's id (1 to {NODE_ID_BYTES_MAX} of a-z, 0-9 and -)"),
            NodesError::Duplicate(id) => write!(f, "FRAGMENT_NODES lists the node {id} twice"),
            NodesError::Url { node, why } => write!(f, "FRAGMENT_NODES: the node {node}: {why}"),
            NodesError::Image { image, why } => write!(f, "FRAGMENT_NODES: the image {image:?}: {why}"),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire<'a> {
    nodes: Vec<WireNode>,
    /// Each image's reference(s), read by `refs`: one for every
    /// architecture, or an object of one per architecture.
    #[serde(borrow)]
    images: BTreeMap<String, &'a RawValue>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireNode {
    id: String,
    url: String,
    arch: Arch,
}

/// An image's reference(s) as the list writes them: a string, the one for
/// every architecture; or an object, one for each architecture it names.
fn refs(raw: &RawValue) -> Result<Vec<(Arch, String)>, String> {
    let text = raw.get().as_bytes();
    if text.first() == Some(&b'"') {
        let every: String = serde_json::from_slice(text).map_err(|e| e.to_string())?;
        return Ok(Arch::ALL.iter().map(|a| (*a, every.clone())).collect());
    }
    let by_arch: BTreeMap<Arch, String> = serde_json::from_slice(text).map_err(|e| format!("a reference, or one per architecture (x86_64, aarch64): {e}"))?;
    Ok(by_arch.into_iter().collect())
}

/// A node's id: 1 to 32 of a-z, 0-9 and `-` (sandcastle's ids are these).
pub fn valid_node_id(id: &str) -> bool {
    (1..=NODE_ID_BYTES_MAX).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// An image's name, as computers are pinned to it.
fn valid_image_name(name: &str) -> bool {
    (1..=IMAGE_NAME_BYTES_MAX).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        && name.as_bytes()[0].is_ascii_alphanumeric()
}

/// A reference pinned by its digest: printable, no spaces, ending
/// `@sha256:` and 64 lowercase hex. The engine parses the rest itself.
pub fn pinned_by_digest(r: &str) -> bool {
    let digest = r.rsplit_once("@sha256:");
    (1..=REFERENCE_BYTES_MAX).contains(&r.len())
        && r.bytes().all(|b| b.is_ascii_graphic())
        && digest.is_some_and(|(name, hex)| !name.is_empty() && hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// The store binding that holds node `id`'s secret: `FRAGMENT_NODE_SECRET_`
/// and the id upper-cased, `-` as `_` (one to one: an id has no `_`).
pub fn secret_binding(id: &str) -> String {
    assert!(valid_node_id(id), "a node's id is checked before its secret is named");
    format!("{SECRET_BINDING_PREFIX}{}", id.to_ascii_uppercase().replace('-', "_"))
}

/// An http(s) origin, its trailing slash dropped.
fn origin(url: &str) -> Result<String, String> {
    if url.len() > URL_BYTES_MAX {
        return Err(format!("a url of at most {URL_BYTES_MAX} bytes"));
    }
    let u = url::Url::parse(url).map_err(|e| format!("url: {e}"))?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() || u.path() != "/" || u.query().is_some() || u.fragment().is_some() || !u.username().is_empty() || u.password().is_some() {
        return Err("its url is an http(s) origin, with no path, query or credentials".into());
    }
    Ok(url.trim_end_matches('/').to_string())
}

impl Nodes {
    /// `FRAGMENT_NODES`, checked.
    pub fn parse(json: &str) -> Result<Nodes, NodesError> {
        let wire: Wire = serde_json::from_slice(json.as_bytes()).map_err(|e| NodesError::Shape(e.to_string()))?;
        if wire.nodes.is_empty() || wire.nodes.len() > NODES_MAX {
            return Err(NodesError::Count(wire.nodes.len()));
        }
        let mut nodes: Vec<Node> = Vec::with_capacity(wire.nodes.len());
        for n in wire.nodes {
            if !valid_node_id(&n.id) {
                return Err(NodesError::Id(n.id));
            }
            if nodes.iter().any(|m| m.id == n.id) {
                return Err(NodesError::Duplicate(n.id));
            }
            let url = origin(&n.url).map_err(|why| NodesError::Url { node: n.id.clone(), why })?;
            nodes.push(Node { id: n.id, url, arch: n.arch });
        }
        if wire.images.is_empty() || wire.images.len() > IMAGES_MAX {
            return Err(NodesError::Image { image: String::new(), why: format!("1 to {IMAGES_MAX} images, not {}", wire.images.len()) });
        }
        let mut images = Vec::with_capacity(wire.images.len());
        for (name, raw) in wire.images {
            let bad = |why: String| NodesError::Image { image: name.clone(), why };
            if !valid_image_name(&name) {
                return Err(bad(format!("a name is 1 to {IMAGE_NAME_BYTES_MAX} of a-z, 0-9 and ._-")));
            }
            let by_arch = refs(raw).map_err(bad)?;
            if by_arch.is_empty() {
                return Err(bad("a reference, or one for at least one architecture".into()));
            }
            if let Some((_, r)) = by_arch.iter().find(|(_, r)| !pinned_by_digest(r)) {
                return Err(bad(format!("{r:?} is not pinned by its digest (<name>@sha256:<64 hex>, at most {REFERENCE_BYTES_MAX} bytes)")));
            }
            images.push((name, by_arch));
        }
        Ok(Nodes { nodes, images })
    }

    /// The nodes, in the deployment's order.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn get(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// The image names computers may be pinned to.
    pub fn image_names(&self) -> Vec<&str> {
        self.images.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// What `image` is on a node of `arch`, if the deployment names one.
    pub fn reference(&self, image: &str, arch: Arch) -> Option<&str> {
        let (_, refs) = self.images.iter().find(|(n, _)| n == image)?;
        refs.iter().find(|(a, _)| *a == arch).map(|(_, r)| r.as_str())
    }

    /// What `image` is on the node a computer is placed on (`placed`), or,
    /// not placed yet, on the first node listed that can run it: the
    /// version its next start runs.
    pub fn reference_on(&self, image: &str, placed: Option<&str>) -> Option<&str> {
        match placed {
            Some(id) => self.reference(image, self.get(id)?.arch),
            None => self.nodes.iter().find_map(|n| self.reference(image, n.arch)),
        }
    }

    /// What the cell's JavaScript needs of node `id` to run a computer on
    /// it (entry.mjs `pin`): its origin, the binding of its secret, and the
    /// images it runs by name. None for a node the deployment no longer lists.
    pub fn pin(&self, id: &str, container: &str) -> Option<Pin> {
        let node = self.get(id)?;
        let images = self.images.iter().filter_map(|(name, _)| self.reference(name, node.arch).map(|r| (name.clone(), r.to_string()))).collect();
        Some(Pin { id: node.id.clone(), url: node.url.clone(), secret: secret_binding(&node.id), container: container.to_string(), images })
    }

    /// Every node as a probe asks it (entry.mjs `probe`).
    pub fn probes(&self) -> Vec<Asked> {
        self.nodes.iter().map(|n| Asked { id: n.id.clone(), url: n.url.clone(), secret: secret_binding(&n.id) }).collect()
    }
}

/// A computer's node, as entry.mjs's `ContainerHost` takes it (`pin`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pin {
    pub id: String,
    pub url: String,
    /// The binding of its secret.
    pub secret: String,
    /// The container's name on the node: the computer's id.
    pub container: String,
    /// Each image it runs: its name and reference, as pairs (entry.mjs
    /// makes them an object).
    pub images: Vec<(String, String)>,
}

/// A node as a probe asks it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Asked {
    pub id: String,
    pub url: String,
    pub secret: String,
}

/// A node's answer to a probe (entry.mjs `probe`): its health within its
/// bound, or why not.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    pub id: String,
    /// Why it is down: no answer within the bound, or a refusal.
    #[serde(default)]
    pub down: Option<String>,
    /// The architecture it reports.
    #[serde(default)]
    pub arch: Option<String>,
    /// The VMs its engine runs now, and at most.
    #[serde(default)]
    pub vms: Option<u32>,
    #[serde(default, rename = "vmsMax")]
    pub vms_max: Option<u32>,
}

/// The node a computer pinned to `image` is placed on (the module's rule),
/// or why none: each node's reason, in the list's order.
pub fn choose<'a>(nodes: &'a Nodes, image: &str, probes: &[Probe]) -> Result<&'a str, String> {
    // the most room so far, and its node: a tie keeps the one listed first
    let mut best: Option<(u32, &str)> = None;
    let mut why: Vec<String> = vec![];
    for node in &nodes.nodes {
        let probe = probes.iter().find(|p| p.id == node.id);
        let reason = match probe {
            None => Some("not probed".to_string()),
            Some(Probe { down: Some(down), .. }) => Some(format!("down: {down}")),
            Some(p) if p.arch.as_deref() != Some(node.arch.name()) => Some(format!("it reports {}, not {} as listed", p.arch.as_deref().unwrap_or("no architecture"), node.arch.name())),
            Some(_) if nodes.reference(image, node.arch).is_none() => Some(format!("the deployment names no {image} image for {}", node.arch.name())),
            Some(Probe { vms: Some(vms), vms_max: Some(max), .. }) if vms >= max => Some(format!("full: {vms} of {max} VMs")),
            Some(Probe { vms: Some(vms), vms_max: Some(max), .. }) => {
                if best.is_none_or(|(room, _)| max - vms > room) {
                    best = Some((max - vms, &node.id));
                }
                None
            }
            Some(_) => Some("its health names no VMs".to_string()),
        };
        if let Some(r) = reason {
            why.push(format!("{}: {r}", node.id));
        }
    }
    best.map(|(_, id)| id).ok_or_else(|| format!("no node can run it ({})", why.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(c: char) -> String {
        std::iter::repeat_n(c, 64).collect()
    }

    fn list() -> String {
        serde_json::json!({
            "nodes": [
                { "id": "box", "url": "http://127.0.0.1:9400/", "arch": "x86_64" },
                { "id": "mac-1", "url": "https://mac.lan:9400", "arch": "aarch64" }
            ],
            "images": {
                "stub": format!("docker.io/library/fragment-stub@sha256:{}", digest('a')),
                "hermes": { "x86_64": format!("registry.lan:5000/fragment-hermes@sha256:{}", digest('b')) }
            }
        })
        .to_string()
    }

    fn up(id: &str, arch: &str, vms: u32, max: u32) -> Probe {
        Probe { id: id.into(), down: None, arch: Some(arch.into()), vms: Some(vms), vms_max: Some(max) }
    }

    /// Goal: a good list reads back as written, its URLs without their
    /// trailing slash; each image's reference is per architecture, one for
    /// all of them standing for each.
    #[test]
    fn a_list_reads() {
        let n = Nodes::parse(&list()).unwrap();
        assert_eq!(n.nodes().iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["box", "mac-1"]);
        assert_eq!(n.get("box").unwrap().url, "http://127.0.0.1:9400");
        assert_eq!(n.image_names(), ["hermes", "stub"]);
        assert!(n.reference("stub", Arch::Aarch64).unwrap().ends_with(&digest('a')));
        assert!(n.reference("hermes", Arch::Aarch64).is_none());
        // not placed: the first node that can run it names its version
        assert!(n.reference_on("hermes", None).unwrap().ends_with(&digest('b')));
        assert_eq!(n.reference_on("hermes", Some("mac-1")), None);
        assert_eq!(n.reference_on("stub", Some("gone")), None);
    }

    /// Goal: every malformed list is refused, saying what is wrong: its
    /// shape, its count, an id, a duplicate, a URL, and an image's name or
    /// reference (one not pinned by its digest above all).
    #[test]
    fn a_bad_list_is_refused() {
        let with = |f: &dyn Fn(&mut serde_json::Value)| {
            let mut v: serde_json::Value = serde_json::from_str(&list()).unwrap();
            f(&mut v);
            Nodes::parse(&v.to_string()).unwrap_err()
        };
        assert!(matches!(Nodes::parse("[]").unwrap_err(), NodesError::Shape(_)));
        assert!(matches!(with(&|v| v["nodes"] = serde_json::json!([])), NodesError::Count(0)));
        let many: Vec<serde_json::Value> = (0..=NODES_MAX).map(|i| serde_json::json!({ "id": format!("n{i}"), "url": "http://a", "arch": "x86_64" })).collect();
        assert!(matches!(with(&|v| v["nodes"] = serde_json::json!(many)), NodesError::Count(17)));
        assert!(matches!(with(&|v| v["nodes"][0]["id"] = "Box".into()), NodesError::Id(_)));
        assert!(matches!(with(&|v| v["nodes"][0]["id"] = "x".repeat(NODE_ID_BYTES_MAX + 1).into()), NodesError::Id(_)));
        assert!(matches!(with(&|v| v["nodes"][1]["id"] = "box".into()), NodesError::Duplicate(_)));
        for url in ["ftp://a", "http://a/path", "http://a?q", "http://u:p@a", "not a url"] {
            assert!(matches!(with(&|v| v["nodes"][0]["url"] = url.into()), NodesError::Url { .. }), "{url}");
        }
        assert!(matches!(with(&|v| v["nodes"][0]["arch"] = "sparc".into()), NodesError::Shape(_)));
        assert!(matches!(with(&|v| v["nodes"][0]["capacity"] = 4.into()), NodesError::Shape(_)), "an unknown field");
        assert!(matches!(with(&|v| v["images"] = serde_json::json!({})), NodesError::Image { .. }));
        assert!(matches!(with(&|v| v["images"]["Stub"] = v["images"]["stub"].clone()), NodesError::Image { .. }));
        for reference in [
            "docker.io/library/fragment-stub:local".to_string(),
            format!("fragment-stub@sha256:{}", digest('A')),
            format!("fragment-stub@sha256:{}", &digest('a')[1..]),
            format!("@sha256:{}", digest('a')),
            format!("fragment stub@sha256:{}", digest('a')),
        ] {
            let e = with(&|v| v["images"]["stub"] = reference.clone().into());
            assert!(e.to_string().contains("pinned by its digest"), "{reference}: {e}");
        }
        assert!(matches!(with(&|v| v["images"]["hermes"] = serde_json::json!({})), NodesError::Image { .. }));
        assert!(matches!(with(&|v| v["images"]["hermes"] = serde_json::json!({ "sparc": format!("x@sha256:{}", digest('b')) })), NodesError::Image { .. }));
        assert!(matches!(with(&|v| v["images"]["hermes"] = serde_json::json!(7)), NodesError::Image { .. }));
    }

    /// Goal: a node's secret is bound by a name the id names alone, one to
    /// one.
    #[test]
    fn a_nodes_secret_is_named_by_its_id() {
        assert_eq!(secret_binding("box"), "FRAGMENT_NODE_SECRET_BOX");
        assert_eq!(secret_binding("mac-1"), "FRAGMENT_NODE_SECRET_MAC_1");
        let n = Nodes::parse(&list()).unwrap();
        let pin = n.pin("mac-1", "computer:00").unwrap();
        assert_eq!((pin.url.as_str(), pin.secret.as_str(), pin.container.as_str()), ("https://mac.lan:9400", "FRAGMENT_NODE_SECRET_MAC_1", "computer:00"));
        // a node runs the images of its architecture alone
        assert_eq!(pin.images.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["stub"]);
        assert_eq!(n.pin("box", "c").unwrap().images.len(), 2);
        assert!(n.pin("gone", "c").is_none());
        assert_eq!(n.probes().iter().map(|a| a.secret.as_str()).collect::<Vec<_>>(), ["FRAGMENT_NODE_SECRET_BOX", "FRAGMENT_NODE_SECRET_MAC_1"]);
    }

    /// Goal: the rule. The most room first, a tie to the node listed
    /// first; a node down, of another architecture than listed, with no
    /// image for its own, full, or silent on its VMs is passed over; with
    /// none left, each node's reason is said.
    #[test]
    fn the_rule() {
        let n = Nodes::parse(&list()).unwrap();
        let both = [up("box", "x86_64", 3, 16), up("mac-1", "aarch64", 1, 8)];
        assert_eq!(choose(&n, "stub", &both).unwrap(), "box", "13 free before 7");
        let even = [up("box", "x86_64", 8, 16), up("mac-1", "aarch64", 0, 8)];
        assert_eq!(choose(&n, "stub", &even).unwrap(), "box", "a tie to the first listed");
        let roomier = [up("box", "x86_64", 15, 16), up("mac-1", "aarch64", 0, 8)];
        assert_eq!(choose(&n, "stub", &roomier).unwrap(), "mac-1");
        // hermes is the box's alone
        assert_eq!(choose(&n, "hermes", &roomier).unwrap(), "box");
        let e = choose(&n, "hermes", &[up("box", "x86_64", 16, 16), up("mac-1", "aarch64", 0, 8)]).unwrap_err();
        assert!(e.contains("box: full: 16 of 16 VMs") && e.contains("mac-1: the deployment names no hermes image for aarch64"), "{e}");
        let down = Probe { id: "box".into(), down: Some("no answer within 6 s".into()), arch: None, vms: None, vms_max: None };
        let e = choose(&n, "stub", &[down.clone(), up("mac-1", "x86_64", 0, 8)]).unwrap_err();
        assert!(e.contains("box: down: no answer within 6 s") && e.contains("mac-1: it reports x86_64, not aarch64 as listed"), "{e}");
        assert_eq!(choose(&n, "stub", &[down, up("mac-1", "aarch64", 7, 8)]).unwrap(), "mac-1");
        let silent = Probe { vms: None, ..up("box", "x86_64", 0, 0) };
        assert!(choose(&n, "stub", &[silent]).unwrap_err().contains("box: its health names no VMs"));
        assert!(choose(&n, "stub", &[]).unwrap_err().contains("box: not probed"));
    }
}
