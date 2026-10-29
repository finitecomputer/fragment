// An operation really in flight: an indeterminate spinner and its elapsed
// time, never an invented percentage or a stage the server did not report.
export function activity(label, started = Date.now()) {
  const row = document.createElement("span");
  row.className = "activity";
  const spinner = document.createElement("span");
  spinner.className = "activity-spinner";
  spinner.setAttribute("aria-hidden", "true");
  const words = document.createElement("span");
  words.textContent = label;
  words.setAttribute("role", "status");
  const elapsed = document.createElement("span");
  elapsed.className = "activity-elapsed";
  elapsed.setAttribute("aria-hidden", "true");
  row.append(spinner, words, elapsed);
  function tick() {
    if (!row.isConnected) return;
    const seconds = Math.max(0, Math.floor((Date.now() - started) / 1000));
    elapsed.textContent = seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
    setTimeout(tick, 1000);
  }
  setTimeout(tick, 1000);
  return row;
}
