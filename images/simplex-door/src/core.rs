//! SimpleX's core, one per door: started with the bridge's own command line
//! (`run::core_args`, the same flags and reasons as a computer's core), its
//! bot API on loopback read back from `/proc` (`run::listening_port`), and
//! every frame it says handed to the runner's loop, in order.

use std::path::Path;
use std::time::{Duration, Instant};

use fragment_bridge::simplex::{run, wire};
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::Msg;

/// Commands written to the core and not yet sent, at most (the bridge's
/// own bound for one core's writer).
const WRITER_MAX: usize = 64;
/// The end of the core's stderr kept, for why it ended.
const STDERR_TAIL_BYTES: usize = 2_048;

pub struct Core {
    pid: u32,
    pub writer: mpsc::Sender<String>,
    /// When its process started (ms since the Unix epoch): what its chats
    /// say changed before it, an earlier life did (`Input::Conns`).
    pub spawned_ms: u64,
}

/// The core started and its bot API dialed; every frame it says, its
/// process's end (`CoreExited`), and its socket's (`CoreLost`: the process
/// may live on), sent to `to`.
pub async fn start(cfg: &run::Config, dir: &Path, name: &str, to: mpsc::Sender<Msg>) -> Result<Core, String> {
    let mut cmd = run::core_command(&cfg.bin, dir);
    cmd.args(run::core_args(cfg, &dir.join("sx"), dir, name, None));
    let mut child = cmd.spawn().map_err(|e| format!("the core did not start: {e}"))?;
    let pid = child.id().ok_or("the core ended at once")?;
    let spawned_ms = fragment_bridge::log::now_ms();
    let stderr = child.stderr.take();
    let ended = to.clone();
    tokio::spawn(async move {
        let mut tail = Vec::new();
        if let Some(mut err) = stderr {
            let mut buf = [0u8; 1024];
            // bounded by the process: its stderr ends with it
            while let Ok(n) = err.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > STDERR_TAIL_BYTES {
                    tail.drain(..tail.len() - STDERR_TAIL_BYTES);
                }
            }
        }
        let status = child.wait().await.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string());
        let last = String::from_utf8_lossy(&tail).lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
        let _ = ended.send(Msg::CoreExited(format!("it ended ({status}): {}", wire::cut(&last, 200)))).await;
    });
    let t0 = Instant::now();
    // bounded by CONNECT_MS_MAX
    let ws = loop {
        let dialed = match run::listening_port(pid) {
            Some(port) => tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}")).await.map(|(ws, _)| ws).map_err(|e| e.to_string()),
            None => Err("its bot API is not listening".to_string()),
        };
        match dialed {
            Ok(ws) => break ws,
            Err(_) if t0.elapsed() < Duration::from_millis(run::CONNECT_MS_MAX) => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(e) => return Err(format!("its bot API: {e}")),
        }
    };
    let (writer, mut writes) = mpsc::channel::<String>(WRITER_MAX);
    let (mut sink, mut stream) = ws.split();
    tokio::spawn(async move {
        // bounded by the socket, which closes with the process
        loop {
            tokio::select! {
                w = writes.recv() => match w {
                    Some(frame) => if sink.send(Message::text(frame)).await.is_err() { break },
                    None => return,
                },
                m = stream.next() => match m {
                    Some(Ok(Message::Text(t))) if t.len() <= wire::FRAME_MAX_BYTES => {
                        if to.send(Msg::Frame(t.to_string())).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(Message::Text(t))) => fragment_bridge::ev!("door.frame_too_large", { "bytes": t.len() }),
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                },
            }
        }
        let _ = to.send(Msg::CoreLost).await;
    });
    Ok(Core { pid, writer, spawned_ms })
}

impl Core {
    /// A core that is the test's own: what the runner writes to it goes to
    /// `writer`, and no process is signalled.
    #[cfg(test)]
    pub fn scripted(writer: mpsc::Sender<String>) -> Core {
        Core { pid: 0, writer, spawned_ms: 0 }
    }

    /// Asks the core's process to end (SIGTERM), or ends it (SIGKILL): its
    /// end is told as any is (`Msg::CoreExited`).
    pub fn signal(&self, kill: bool) {
        let sig = if kill { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: a signal to the core's own process, by its pid, which this
        // runner reaps (its exit watcher) only after it has ended
        if self.pid > 0 {
            unsafe { libc::kill(self.pid as i32, sig) };
        }
    }
}
