//! pod-init: in the TD, keeps the owner's LUKS volume. After any boot
//! the volume is locked; `/status` attests the state with a boot key
//! made now, and `/init` or `/unlock` open it for a disk key sealed to
//! that boot key in a request the owner signed. See docs/pod.md.

use fragment_pod_init::config::{self, Args, Config};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = match config::parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("pod-init: {e}");
            }
            eprintln!("{}", config::USAGE);
            return ExitCode::from(2);
        }
    };
    let config = match Config::load(&args.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pod-init: {e}");
            return ExitCode::from(2);
        }
    };
    run(args, config)
}

/// configfs-tsm, dm-crypt and the CCEL exist only on a Linux TDX guest.
#[cfg(target_os = "linux")]
fn run(args: Args, config: Config) -> ExitCode {
    use fragment_pod_init::luks::Luks;
    use fragment_pod_init::quoter::Tsm;
    use fragment_pod_init::service::Service;
    use fragment_pod::unlock::keys::BootKey;
    use std::net::{SocketAddr, TcpListener};

    if !args.configfs.is_dir() {
        eprintln!("pod-init: {} is missing: not a TDX guest, or configfs is not mounted", args.configfs.display());
        return ExitCode::FAILURE;
    }
    let boot = match BootKey::generate() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("pod-init: boot key: {e}");
            return ExitCode::FAILURE;
        }
    };
    let addr = SocketAddr::from((args.bind, args.port));
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("pod-init: bind {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("pod-init: listening on {addr}, boot key {}", hex::encode(boot.public()));
    let service = Service {
        volume: Box::new(Luks::new(config.clone())),
        quoter: Box::new(Tsm { report_root: args.configfs }),
        boot,
        ccel: args.ccel,
        hook_timeout: fragment_pod_init::hook::TIMEOUT,
        config,
    };
    service.serve(&listener, None);
    ExitCode::FAILURE
}

#[cfg(not(target_os = "linux"))]
fn run(args: Args, config: Config) -> ExitCode {
    let _ = (args, config);
    eprintln!("pod-init: runs only on a Linux TDX guest (configfs-tsm, dm-crypt)");
    ExitCode::FAILURE
}
