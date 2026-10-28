// Feedback UI owns its form, attachments and request lifecycle.
// The shell supplies shared presentation and its release/write guards.
export function createFeedback({ $, el, icon, section, actionLine, heard, beginWrite }) {
  // ---- help: a problem reported, or a message, to the deployment's operators
  // (docs/api.md, Feedback), with up to three screenshots, picked, pasted or
  // dropped: from Settings' Help, and a chat's options. It names the page it
  // was opened on (a chat's path, or settings').
  const FEEDBACK_SHOTS_MAX = 3;
  const FEEDBACK_SHOT_MAX_BYTES = 5 * 1024 * 1024;
  const FEEDBACK_TYPES = ["image/png", "image/jpeg", "image/webp", "image/gif"];
  const FEEDBACK_LABEL = { problem: ["What happened?", "What did you do, what did you expect, and what happened instead?"], message: ["Your message", "Anything you want us to know"] };
  // `opened` counts the dialog's openings: an answer to a send of an earlier
  // one (sent, then closed and opened again) changes nothing of this one's.
  // `id` is the opening's one send: sent again after an error, it is the
  // same send, so one whose answer was lost is never kept twice.
  const feedback = { shots: [], page: "/", opened: 0, id: "" };
  const feedbackId = () => Array.from(crypto.getRandomValues(new Uint8Array(16)), (b) => b.toString(16).padStart(2, "0")).join("");
  function helpSection() {
    const s = section("Help");
    s.id = "settings-help";
    s.append(el("p", "muted", "Something not working, or something you want us to know? Write to us here, with screenshots if they help. We read every one."));
    const ways = el("span", "settings-actions");
    for (const [kind, text] of [["problem", "Report a problem"], ["message", "Send a message"]]) {
      const b = el("button", "quiet", text);
      b.type = "button";
      b.dataset.action = `feedback-${kind}`;
      b.onclick = () => openFeedback(kind);
      ways.append(b);
    }
    s.append(actionLine("Feedback", ways));
    return s;
  }
  function openFeedback(kind) {
    const form = $("feedback-form");
    form.reset();
    form.elements.kind.value = kind;
    form.hidden = false;
    $("feedback-sent").hidden = true;
    feedback.opened++;
    feedback.id = feedbackId();
    const send = $("feedback-send");
    send.disabled = false;
    send.textContent = "Send";
    feedback.page = location.pathname;
    feedbackShots([]);
    feedbackSaid(null);
    feedbackLabel();
    $("feedback-dialog").showModal();
    $("feedback-message").focus();
  }
  function feedbackLabel() {
    const [label, hint] = FEEDBACK_LABEL[$("feedback-form").elements.kind.value] ?? FEEDBACK_LABEL.problem;
    $("feedback-label").textContent = label;
    $("feedback-message").placeholder = hint;
  }
  function feedbackSaid(text) {
    const said = $("feedback-error");
    said.textContent = text ?? "";
    said.hidden = !text;
  }
  // the screenshots it holds, each a thumbnail with its own Remove
  function feedbackShots(files) {
    for (const s of feedback.shots) URL.revokeObjectURL(s.url);
    feedback.shots = files.map((file) => ({ file, url: URL.createObjectURL(file) }));
    $("feedback-shots").replaceChildren(...feedback.shots.map((s, i) => {
      const shot = el("div", "feedback-shot");
      const img = el("img");
      img.src = s.url;
      img.alt = `Screenshot ${i + 1}`;
      const remove = el("button");
      remove.type = "button";
      remove.setAttribute("aria-label", `Remove screenshot ${i + 1}`);
      remove.append(icon("x"));
      remove.onclick = () => feedbackShots(feedback.shots.filter((_, j) => j !== i).map((x) => x.file));
      shot.append(img, remove);
      return shot;
    }));
    const full = feedback.shots.length >= FEEDBACK_SHOTS_MAX;
    $("feedback-file").disabled = full;
    $("feedback-attach").setAttribute("aria-disabled", String(full));
  }
  // screenshots added: what is no image, too large, or past the third is
  // said, and the rest kept
  function addFeedbackShots(files) {
    const kept = feedback.shots.map((s) => s.file);
    let refused = null;
    for (const f of files) {
      if (!FEEDBACK_TYPES.includes(f.type)) refused = `${f.name || "That file"} is not an image: a PNG, JPEG, WebP or GIF.`;
      else if (f.size > FEEDBACK_SHOT_MAX_BYTES) refused = `${f.name || "That image"} is larger than 5 MB: crop it, or send a smaller one.`;
      else if (kept.length >= FEEDBACK_SHOTS_MAX) refused = `At most ${FEEDBACK_SHOTS_MAX} screenshots.`;
      else kept.push(f);
    }
    feedbackShots(kept);
    feedbackSaid(refused);
  }
  $("feedback-form").elements.kind.forEach((r) => (r.onchange = feedbackLabel));
  $("feedback-file").onchange = () => {
    addFeedbackShots([...$("feedback-file").files]);
    $("feedback-file").value = "";
  };
  $("feedback-message").addEventListener("paste", (e) => {
    const images = [...(e.clipboardData?.files ?? [])].filter((f) => f.type.startsWith("image/"));
    if (!images.length) return;
    e.preventDefault();
    addFeedbackShots(images);
  });
  $("feedback-dialog").addEventListener("dragover", (e) => {
    if (![...(e.dataTransfer?.types ?? [])].includes("Files")) return;
    e.preventDefault();
    $("feedback-dialog").classList.add("dropping");
  });
  $("feedback-dialog").addEventListener("dragleave", (e) => {
    if (!$("feedback-dialog").contains(e.relatedTarget)) $("feedback-dialog").classList.remove("dropping");
  });
  $("feedback-dialog").addEventListener("drop", (e) => {
    $("feedback-dialog").classList.remove("dropping");
    if (!e.dataTransfer?.files.length) return;
    e.preventDefault();
    addFeedbackShots([...e.dataTransfer.files]);
  });
  // a close event comes after the close (a task queued), so one of an opening
  // closed and opened again at once is the earlier one's: it clears nothing
  $("feedback-dialog").addEventListener("close", () => {
    if (!$("feedback-dialog").open) feedbackShots([]);
  });
  $("feedback-cancel").onclick = () => $("feedback-dialog").close();
  $("feedback-done").onclick = () => $("feedback-dialog").close();
  $("feedback-form").onsubmit = async (e) => {
    e.preventDefault();
    const form = $("feedback-form");
    const opened = feedback.opened;
    const body = new FormData();
    body.append("id", feedback.id);
    body.append("kind", form.elements.kind.value);
    body.append("message", $("feedback-message").value);
    body.append("page", feedback.page);
    for (const s of feedback.shots) body.append("screenshot", s.file, s.file.name || "screenshot");
    const send = $("feedback-send");
    send.disabled = true;
    send.textContent = "Sending…";
    feedbackSaid(null);
    const endWrite = beginWrite();
    let r = null;
    try {
      r = await fetch("/api/feedback", { method: "POST", headers: { "x-fragment-shell": "1" }, body, credentials: "same-origin" });
      heard(r);
    } catch {
      // no answer: said below, and Send stays to try again
    } finally {
      endWrite();
    }
    // closed, or opened again, meanwhile: the answer is an earlier opening's
    if (opened !== feedback.opened || !$("feedback-dialog").open) return;
    send.disabled = false;
    send.textContent = "Send";
    // that send is gone (taken too long, or deleted): another try is a new one
    if (r?.status === 404 || r?.status === 409) feedback.id = feedbackId();
    if (r?.ok) {
      feedbackShots([]);
      form.hidden = true;
      $("feedback-sent").hidden = false;
      $("feedback-done").focus();
      return;
    }
    const said = r ? (await r.json().catch(() => ({}))).message || "That wasn't sent. Try again." : "We couldn't be reached. Check your connection, and try again.";
    if (opened === feedback.opened) feedbackSaid(said);
  };
  return { openFeedback, helpSection };
}
