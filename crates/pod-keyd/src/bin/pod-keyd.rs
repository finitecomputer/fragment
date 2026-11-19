//! pod-keyd: in a TD of its own, holds owners' pod disk keys in memory
//! and releases one to a boot of that pod whose quote passes the
//! owner's checks. Holds nothing at start but the names of the owners
//! it serves; see docs/pod.md.

use fragment_pod_keyd::args::{self, Args};
use std::process::ExitCode;

fn main() -> ExitCode {
    match args::parse(std::env::args().skip(1)) {
        Ok(a) => run(a),
        Err(e) => {
            if !e.is_empty() {
                eprintln!("pod-keyd: {e}");
            }
            eprintln!("{}", args::USAGE);
            ExitCode::from(2)
        }
    }
}

/// configfs-tsm and the CCEL exist only on a Linux TDX guest.
#[cfg(target_os = "linux")]
fn run(args: Args) -> ExitCode {
    use fragment_pod::checks::attestor::Live;
    use fragment_pod_keyd::clock::System;
    use fragment_pod_keyd::nonces::Challenges;
    use fragment_pod_keyd::service::Service;
    use fragment_pod_init::quoter::Tsm;
    use fragment_pod::unlock::keys::BootKey;
    use std::net::{SocketAddr, TcpListener};

    if !args.configfs.is_dir() {
        eprintln!("pod-keyd: {} is missing: not a TDX guest, or configfs is not mounted", args.configfs.display());
        return ExitCode::FAILURE;
    }
    let (boot, challenges) = match (BootKey::generate(), Challenges::generate()) {
        (Ok(b), Ok(c)) => (b, c),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("pod-keyd: boot keys: {e}");
            return ExitCode::FAILURE;
        }
    };
    let addr = SocketAddr::from((args.bind, args.port));
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("pod-keyd: bind {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("pod-keyd: listening on {addr}, boot key {}, serving {} owner(s)", hex::encode(boot.public()), args.owners.len());
    let quoter = Box::new(Tsm { report_root: args.configfs });
    let service = Service::new(boot, challenges, args.owners, quoter, Box::new(Live::default()), Box::new(System), args.ccel);
    service.serve(&listener, None);
    ExitCode::FAILURE
}

#[cfg(not(target_os = "linux"))]
fn run(args: Args) -> ExitCode {
    let _ = args;
    eprintln!("pod-keyd: runs only on a Linux TDX guest (configfs-tsm)");
    ExitCode::FAILURE
}
