//! LAN mode (crates/lan; docs/self-host.md, seam 9): the node started
//! again as a self-hosted box serves it (`Shape::Lan`), its platform at
//! `https://<LAN_ZONE>:<port>` and its fragments under the zone, behind
//! fragment-lan's door, with no code of the cell's own for it.
//!
//! Goal: a device that asks the box's DNS and trusts the box's root alone
//! signs in, makes and opens a fragment at its own https origin; the zone
//! answers its names and refuses others; what is no fragment's or no
//! zone's is refused; the door names each client's address, and one a
//! client names itself is never counted; the root survives a restart and
//! the leaf does not; nothing listens on another address.
//! Method: fragment-lan in-process on loopback (the e2e's ports picked
//! free), the e2e's client resolving through its DNS and trusting its root
//! (`Api::fronted`), the node's own port asked directly beside it; the
//! service dropped and started again on its ports; the node started plain
//! again after.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use fragment_lan::ca::Opened;
use fragment_lan::dns::{lookup, Ask, Heard};
use fragment_lan::zone::https_origin;
use fragment_lan::{Config, Lan};
use fragment_nip98::Keys;

use crate::api::{Api, Call};
use crate::{Suite, LAN_ZONE};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
/// How long a DNS question waits: the box is on loopback.
const DNS_WAIT: Duration = Duration::from_secs(2);

/// A device's resolver for the box's names: it asks the box's DNS, as a
/// tailnet's split DNS sends the zone's questions there.
struct BoxDns(SocketAddr);

impl reqwest::dns::Resolve for BoxDns {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let (server, name) = (self.0, name.as_str().to_string());
        Box::pin(async move {
            match lookup(server, &name, Ask::A, DNS_WAIT) {
                Ok(Heard::Addresses(found)) if !found.is_empty() => Ok(Box::new(found.into_iter().map(|ip| SocketAddr::new(ip, 0))) as reqwest::dns::Addrs),
                heard => Err(format!("the box's DNS for {name}: {heard:?}").into()),
            }
        })
    }
}

/// A device on the box's network: it resolves by the box's DNS, trusts its
/// root alone, and keeps no connection (as `Api`'s local client).
fn device(lan: &Lan, tls_info: bool) -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(0)
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_der(&lan.root_der)?)
        .dns_resolver(Arc::new(BoxDns(lan.dns.context("LAN mode answers DNS")?)))
        .tls_info(tls_info)
        .build()?)
}

fn config(s: &Suite, ports: (u16, u16, u16)) -> Config {
    Config {
        zone: LAN_ZONE.into(),
        address: LOOPBACK,
        https_port: ports.0,
        http_port: Some(ports.1),
        dns_port: Some(ports.2),
        node: SocketAddr::new(LOOPBACK, s.port),
        state_dir: s.scratch.join("lan-state"),
    }
}

fn ports(lan: &Lan) -> (u16, u16, u16) {
    (lan.https.port(), lan.http.map_or(0, |a| a.port()), lan.dns.map_or(0, |a| a.port()))
}

pub fn lan(s: &mut Suite, _api: &Api) -> Result<()> {
    // the node itself (restarted as a box); its CLI codes counted by
    // address (the deployment's limit); a fragment's files (the fake)
    if !s.section("lan", &[crate::Need::Fakes, crate::Need::Node, crate::Need::Deployment, crate::Need::Levers]) {
        return Ok(());
    }
    let lan = Lan::start(config(s, (0, 0, 0)))?;
    let platform = https_origin(LAN_ZONE, lan.https.port());
    s.stop()?;
    let checked = s.start_as_lan_box(&platform).and_then(|direct| cases(s, &direct, lan, &platform));
    // the node as the sections after expect it, whatever the cases did
    s.stop()?;
    s.start(false)?;
    checked
}

fn cases(s: &mut Suite, direct: &Api, lan: Lan, platform: &str) -> Result<()> {
    let state = s.scratch.join("lan-state");
    s.ok(
        "a box's first start makes its root, kept in its state directory (the key owner-only)",
        lan.opened == Opened::Made && std::fs::read_to_string(state.join(fragment_lan::ca::ROOT_PEM)).is_ok_and(|p| p == lan.root_pem),
        format!("{:?} in {}", lan.opened, state.display()),
    );

    // the zone's names, from the box's DNS
    let dns = lan.dns.context("LAN mode answers DNS")?;
    let ask = |name: &str| lookup(dns, name, Ask::A, DNS_WAIT);
    let fragment_host = format!("todo--k8ew.{LAN_ZONE}");
    s.ok(
        "the box's DNS answers the platform's name and a fragment's with the box's address",
        matches!(ask(LAN_ZONE), Ok(Heard::Addresses(a)) if a == [LOOPBACK]) && matches!(ask(&fragment_host), Ok(Heard::Addresses(a)) if a == [LOOPBACK]),
        format!("{:?} / {:?}", ask(LAN_ZONE), ask(&fragment_host)),
    );
    s.ok(
        "and refuses every other name (it forwards nothing), a deeper one not there",
        ["example.com", "evillan.home.arpa"].iter().all(|n| matches!(ask(n), Ok(Heard::Refused))) && matches!(ask(&format!("a.b.{LAN_ZONE}")), Ok(Heard::NoSuchName)),
        format!("{:?} / {:?}", ask("example.com"), ask(&format!("a.b.{LAN_ZONE}"))),
    );

    // the root, offered over plain http by the box's address
    let http = lan.http.context("LAN mode serves the root's page")?;
    let plain = reqwest::blocking::Client::builder().redirect(reqwest::redirect::Policy::none()).build()?;
    let page = plain.get(format!("http://{http}/")).send()?.text()?;
    let pem = plain.get(format!("http://{http}{}", fragment_lan::door::ROOT_PEM_PATH)).send()?.text()?;
    s.ok("the root's page, by the box's address over http, shows its fingerprint, and offers the root itself", page.contains(&lan.fingerprint) && pem == lan.root_pem, &lan.fingerprint);

    // the platform, through the door, as a device that trusts the root
    let front = Api::fronted(platform, LAN_ZONE, device(&lan, false)?, &s.shared)?;
    let health = front.unsigned("GET", "/healthz", None)?;
    s.ok(
        "the platform answers at https://<zone>, through the door, to a device that trusts the box's root alone",
        health.status == 200 && health.text == "ok" && !health.header("x-fragment-deploy").is_empty(),
        &health,
    );
    let start = front.unsigned("GET", "/auth/login?return=/", None)?;
    let login = start.headers.get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).find(|c| c.contains("fragment_login=")).unwrap_or("").to_string();
    s.ok("the cell sees an https origin: its sign-in cookie is __Host-, Secure", login.starts_with("__Host-fragment_login=") && login.contains("Secure"), &login);
    let owner = front.person()?;
    let me = front.signed(&owner, "GET", "/api/identities/me", None)?;
    s.ok("a person signs in and approves a key through the door; signed calls verify at https://<zone>", me.status == 200 && me.body["kind"] == "person", &me);

    // a fragment at its own origin, by name
    let name = s.named(&front, &owner, "lan")?;
    let c = s.create(&front, &owner, &name)?;
    s.commit(&c, &[("site/index.html", Some(b"<!doctype html><p>served on the LAN</p>"))]);
    s.deploy(&c);
    let view = c["viewToken"].as_str().unwrap_or("").to_string();
    let page = front.page(&name, &format!("?view={view}"), None)?;
    s.ok(
        "a fragment is served at https://<name>.<zone>, its own origin, through the door",
        page.status == 200 && page.text.contains("served on the LAN") && front.site_url(&name, "").starts_with(&format!("https://{name}.{LAN_ZONE}:")),
        &page,
    );
    let nobody = front.page("nosuch--aaaa", "", None)?;
    s.ok("a name in the zone that is no fragment's reaches the cell, which says so (404)", nobody.status == 404 && nobody.message().contains("no such fragment"), &nobody);
    let elsewhere = front.call(Call { method: "GET", url: format!("{platform}/healthz"), extra: vec![("host", "bank.example".into())], ..Call::default() })?;
    s.ok("a name outside the zone is refused at the door (421), never the node's", elsewhere.status == 421 && !elsewhere.text.contains("ok"), &elsewhere);

    addresses_counted(s, direct, &front)?;
    let lan = restarted(s, lan, platform)?;
    nothing_elsewhere(s, &lan);
    drop(lan);
    Ok(())
}

/// The door names each client's address (`CF-Connecting-IP`, docs/self-host.md
/// found 48): the CLI's codes an address may ask for are counted by the
/// connection's peer, and an address a client names itself is dropped.
fn addresses_counted(s: &mut Suite, direct: &Api, front: &Api) -> Result<()> {
    let limit = crate::CLI_BEGINS_PER_IP_MINUTE as usize;
    // the owner's key approved through the door this minute asked one code
    // from the same peer already
    let asked_before = 1;
    let flood = Keys::generate();
    // bounded: across a minute's turn the limit is passed by twice it at most;
    // a second round in case the minute turned before the direct check
    for round in 0..2 {
        let mut statuses = vec![];
        while statuses.len() <= 2 * limit && statuses.last() != Some(&429) {
            // a new address named each time: were it counted, none would be refused
            let named = format!("198.51.100.{}", statuses.len() % 250 + 1);
            statuses.push(front.cli_begin_from(&flood, Some(&named))?.status);
        }
        let refused_at = statuses.iter().position(|st| *st == 429);
        let peer = direct.cli_begin_from(&flood, Some("127.0.0.1"))?;
        let named = direct.cli_begin_from(&flood, Some("198.51.100.1"))?;
        if peer.status != 429 && round == 0 {
            continue;
        }
        s.ok(
            &format!("through the door, CLI codes are counted by the client's own address: past {limit} a minute it is refused, whatever address it names"),
            refused_at.is_some_and(|i| (limit - asked_before..=2 * limit).contains(&i)),
            format!("refused first at {refused_at:?} of {}", statuses.len()),
        );
        s.ok(
            "the bucket was the connection's peer (127.0.0.1, refused), and the address a client named never counted (200)",
            peer.status == 429 && named.status == 200,
            format!("{peer} / {named}"),
        );
        return Ok(());
    }
    unreachable!("the second round reports")
}

/// The service dropped and started again on the same ports and state.
fn restarted(s: &mut Suite, lan: Lan, platform: &str) -> Result<Lan> {
    let (root, fingerprint, at) = (lan.root_der.clone(), lan.fingerprint.clone(), ports(&lan));
    let leaf = |lan: &Lan| -> Result<Vec<u8>> {
        let r = device(lan, true)?.get(format!("{platform}/healthz")).send()?;
        let info = r.extensions().get::<reqwest::tls::TlsInfo>().and_then(|i| i.peer_certificate().map(<[u8]>::to_vec));
        info.context("the door's certificate, as the device saw it")
    };
    let first = leaf(&lan)?;
    drop(lan);
    let lan = Lan::start(config(s, at))?;
    let second = leaf(&lan)?;
    s.ok(
        "a restart keeps the root (devices trust it still) and serves a new leaf, its key made anew",
        lan.opened == Opened::Kept && lan.root_der == root && lan.fingerprint == fingerprint && first != second,
        format!("{:?}; the leaf {}", lan.opened, if first == second { "the same" } else { "new" }),
    );
    let front = Api::fronted(platform, LAN_ZONE, device(&lan, false)?, &s.shared)?;
    let health = front.unsigned("GET", "/healthz", None)?;
    s.ok("and the platform answers through it again", health.status == 200, &health);
    Ok(lan)
}

/// Nothing answers on this machine's other addresses: IPv6 loopback, and
/// its own address toward the network (read from a UDP socket's route;
/// nothing is sent).
fn nothing_elsewhere(s: &mut Suite, lan: &Lan) {
    let route = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|u| u.connect("192.0.2.1:9").and_then(|_| u.local_addr()));
    let mut others: Vec<IpAddr> = vec!["::1".parse().expect("an address")];
    others.extend(route.ok().map(|a| a.ip()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified()));
    let mut reached = vec![];
    for other in &others {
        for at in [Some(lan.https), lan.http, lan.dns].into_iter().flatten() {
            let to = SocketAddr::new(*other, at.port());
            if std::net::TcpStream::connect_timeout(&to, Duration::from_secs(1)).is_ok() {
                reached.push(to.to_string());
            }
        }
        if let Some(dns) = lan.dns {
            if lookup(SocketAddr::new(*other, dns.port()), LAN_ZONE, Ask::A, Duration::from_millis(500)).is_ok() {
                reached.push(format!("DNS on {other}"));
            }
        }
    }
    s.ok(&format!("nothing listens on another address of this machine ({} checked)", others.len()), reached.is_empty(), format!("{others:?}: {reached:?}"));
}
