// Exact agent page source, with only its browser boundaries replaced. Hold
// the first subscription hello until main has moved, without a changed frame.
async function agentWatch(run) {
  for (const delayed of [false, true]) {
    let socket, release;
    let job = "You are Bliss.";
    let reads = 0;
    const soul = { textContent: "", classList: { toggle() {} } };
    const document = {
      querySelector: () => ({ content: "XP" }),
      getElementById: (id) => id === "soul" ? soul : { style: { setProperty() {} } },
    };
    const window = { addEventListener() {} };
    window.parent = window;
    const fetch = async (path) => {
      if (path === "__file?path=SOUL.md") {
        reads++;
        const snapshot = job;
        if (reads === 1) return new Promise((resolve) => { release = () => resolve({ ok: true, text: async () => snapshot }); });
        return { ok: true, text: async () => snapshot };
      }
      return { ok: true, text: async () => "{}", json: async () => ({ files: [] }) };
    };
    const boot = run(document, window, fetch, class { constructor() { socket = this; } },
      { hostname: "xp--abcd.example", href: "https://xp--abcd.example/" }, class {}, () => "red");
    // Allow settings and identity reads to finish and the initial job to start.
    await new Promise((resolve) => setTimeout(resolve, 0));
    if (!release || !socket) throw new Error("initial job read and watch did not start");
    if (!delayed) {
      release();
      await boot;
      if (soul.textContent !== job) throw new Error("initial job not shown");
    }
    job = "You are XP.";
    socket.onmessage({ data: JSON.stringify({ type: "hello", sha: "repaired-main" }) });
    await new Promise((resolve) => setTimeout(resolve, 0));
    if (delayed) {
      release();
      await boot;
    }
    if (reads !== 2 || soul.textContent !== job) throw new Error(`first hello missed repair (delayed=${delayed}): ${reads} reads, ${soul.textContent}`);
  }
  return true;
}
