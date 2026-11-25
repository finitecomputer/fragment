// An XFTP relay (fragment_core::relay; docs/computers.md, "The XFTP
// relay"): a client's WebSocket piped to raw TCP to the host Rust allowed
// (`connect()`, which nothing inspects: Containers reset a TLS hello on 443
// that names no server, and SimpleX's servers take their protocol only from
// one). Its clients are a computer's guest (entry.mjs `ComputerEgress`)
// and a SimpleX door's container (entry.mjs `DoorEgress`). Bytes pass
// untouched both ways, the core's TLS end to end; this sees their count
// and times alone, and logs its open and close (`of`: whose it is), never
// what is carried. It ends at its bounds (bytes, time), on either side's
// close or error, and once, telling `closed` what it carried.
// Not piped through Rust: a stream of bytes, nothing in it to decide.
import { connect } from "cloudflare:sockets";

// A frame from its client is at most this (the bridge and a door's runner
// send 16 KiB), and what waits to be written toward the server at most
// this, at once.
const RELAY_FRAME_MAX = 256 * 1024;
const RELAY_QUEUE_MAX = 1024 * 1024;

export function relay(ctx, { host, port, bytesMax, msMax }, of, closed) {
  const [client, server] = Object.values(new WebSocketPair());
  server.accept();
  const t0 = Date.now();
  let bytes = 0;
  // what waits to be written toward the server: bounded on its own, apart
  // from the relay's total (Alex's review): a server that stops reading
  // closes the relay, never piles its client's bytes up in memory
  let queued = 0;
  let ended = false;
  const socket = connect({ hostname: host, port }, { secureTransport: "off", allowHalfOpen: false });
  const writer = socket.writable.getWriter();
  const end = (why, code) => {
    if (ended) return;
    ended = true;
    clearTimeout(timer);
    try { server.close(code, why); } catch {}
    socket.close().catch(() => {});
    console.log(JSON.stringify({ relay: "close", ...of, host, port, bytes, ms: Date.now() - t0, why }));
    ctx.waitUntil(closed(bytes));
  };
  const timer = setTimeout(() => end("its time is up", 4008), msMax);
  const counted = (n) => {
    bytes += n;
    if (bytes <= bytesMax) return true;
    end("its bytes are spent", 4008);
    return false;
  };
  server.addEventListener("message", (e) => {
    if (ended) return;
    const data = typeof e.data === "string" ? new TextEncoder().encode(e.data) : new Uint8Array(e.data);
    // an empty frame carries nothing: no write is queued for it
    if (data.byteLength === 0) return;
    if (data.byteLength > RELAY_FRAME_MAX) return end("a frame past its bound", 4008);
    if (!counted(data.byteLength)) return;
    queued += data.byteLength;
    if (queued > RELAY_QUEUE_MAX) return end("the server is not taking what it is sent", 4008);
    // writes queue in order on the stream; each one written frees its bytes
    writer.write(data).then(() => { queued -= data.byteLength; }, () => end("the server's side failed", 1011));
  });
  server.addEventListener("close", () => end("its client closed it", 1000));
  server.addEventListener("error", () => end("its client's side failed", 1011));
  socket.opened.then(
    () => console.log(JSON.stringify({ relay: "open", ...of, host, port })),
    () => end("the server did not answer", 1011),
  );
  ctx.waitUntil((async () => {
    const reader = socket.readable.getReader();
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done || ended) break;
        if (!counted(value.byteLength)) break;
        server.send(value);
      }
      end("the server closed it", 1000);
    } catch {
      end("the server's side failed", 1011);
    }
  })());
  return new Response(null, { status: 101, webSocket: client });
}
