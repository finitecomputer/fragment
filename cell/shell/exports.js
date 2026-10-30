// ---- your data: an export of everything that is the person's (docs/api.md,
// Export), built in the background, which they are mailed about once it is
// ready, and which downloads from here (the platform session, on a click
// on this page) for seven days. One at a time; three a day.
const EXPORT_STATE = { building: "Building", ready: "Ready to download", failed: "Failed", expired: "Expired" };
const sized = (n) => (n >= 1e9 ? `${(n / 1e9).toFixed(1)} GB` : n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : n >= 1e3 ? `${(n / 1e3).toFixed(1)} KB` : `${n} bytes`);
export function dataSection({ api, el, section, line, actionLine, isOpen }) {
  const s = section("Your data");
  s.id = "settings-data";
  s.append(
    el(
      "p",
      "muted",
      "Download everything that is yours: your fragments and your agents' (their files, app data and messages), your computer's newest save, and the names of what others share with you. It is one file, made in the background: we email you when it is ready, and it is kept for seven days.",
    ),
  );
  // its rows, drawn again as the export changes
  const shown = el("div", "settings-export");
  shown.dataset.state = "loading";
  s.append(shown);
  let timer = null;
  const render = (v, refused) => {
    shown.dataset.state = v?.state ?? "none";
    const go = el("button", "quiet", v?.state === "building" ? "Exporting…" : v?.state === "ready" ? "Export again" : "Export everything");
    go.type = "button";
    go.dataset.action = "export";
    go.disabled = v?.state === "building";
    go.onclick = async () => {
      go.disabled = true;
      try {
        render(await api("POST", "/api/export"));
      } catch (e) {
        render(v, e.message);
      }
    };
    const rows = [];
    if (v?.state === "ready" && v.download) {
      const get = el("a", "quiet", "Download");
      get.href = v.download;
      get.dataset.action = "export-download";
      rows.push(line("Export", EXPORT_STATE.ready, get), line("Size", sized(v.size ?? 0)), line("Kept until", new Date(v.expiresAt).toLocaleString()), line("New export", "Replaces this one, and its download", go));
    } else if (v?.state === "building") {
      rows.push(line("Export", EXPORT_STATE.building, go), line("Progress", `${v.progress.done} of ${v.progress.total} parts, ${sized(v.progress.bytes)} so far`));
    } else {
      rows.push(line("Export", v ? EXPORT_STATE[v.state] ?? v.state : "None yet", go));
      if (v?.state === "failed" && v.error) rows.push(el("p", "settings-warning", v.error.message));
    }
    if (refused) rows.push(el("p", "settings-warning", refused));
    shown.replaceChildren(...rows);
    // while it builds, read it again, as long as settings show this section
    clearTimeout(timer);
    if (v?.state === "building") timer = setTimeout(refresh, 2000);
  };
  // A read that fails keeps the rows shown, adds one line that says so,
  // and stops reading: its Try again reads again (never a new export), and
  // what it reads goes on as any read does (#440).
  const unread = () => {
    const again = el("button", "quiet", "Try again");
    again.type = "button";
    again.dataset.action = "export-read";
    again.onclick = () => {
      again.disabled = true;
      refresh();
    };
    const said = actionLine(el("span", "settings-warning", "Couldn't check your export."), again);
    said.dataset.unread = "";
    shown.querySelector("[data-unread]")?.remove();
    shown.append(said);
  };
  const refresh = () => {
    // only while settings show it (its first read is before it is placed);
    // settings shown again build it again, which reads again
    if (shown.dataset.state !== "loading" && (!s.isConnected || !isOpen())) return;
    api("GET", "/api/export").then((r) => render(r.export), unread);
  };
  refresh();
  return s;
}
