//! Old-image discovery translation. Remove only after active and retained
//! rollback images no longer require ?simplex=1 or ComputerView.simplex.
use fragment_proto::computer::{ComputerCapabilities, ConnectorPage};

pub fn capabilities(simplex: bool) -> Option<ComputerCapabilities> {
    simplex.then(|| ComputerCapabilities {
        connectors: vec![ConnectorPage { id: "simplex".into(), title: "SimpleX".into(), port: 6080, path: "/simplex/".into(), requires_always_on: true }],
        ..ComputerCapabilities::default()
    })
}

pub fn simplex(capabilities: Option<&ComputerCapabilities>, old: bool) -> bool {
    capabilities.map_or(old, |c| c.connectors.iter().any(|p| p.id == "simplex"))
}
