// A modal card closes on a press outside it, never on one in its padding or a
// selection dragged past its edge, which took typing with it: the press and
// its click both outside the card (Chrome's Modern Web Guidance, `dialog`).
// It closes on the click, as it did, so the modal takes the whole tap and
// none reaches the page behind: `closedby="any"` would close it on the
// release, before a touch's click (HTML's light dismiss), and Safari has none.
export function closeOnOutsidePress(dialog) {
  const outside = (e) => {
    const r = dialog.getBoundingClientRect();
    return e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom;
  };
  let pressedOutside = false;
  dialog.addEventListener("pointerdown", (e) => { pressedOutside = outside(e); }, true);
  dialog.addEventListener("click", (e) => { if (e.target === dialog && pressedOutside && outside(e)) dialog.close(); });
}
