// Pointer-following rim from the onboarding card. No account interactions.
(() => {
  const card = document.querySelector('.platform-page main');
  const pointer = matchMedia('(hover:hover) and (pointer:fine)');
  if (!card) return;
  let frame = 0;
  let point;
  function clear() {
    cancelAnimationFrame(frame);
    frame = 0;
    card.style.setProperty('--edge-light', '0');
  }
  document.addEventListener('pointermove', event => {
    if (!pointer.matches || event.pointerType === 'touch' || document.documentElement.classList.contains('framed')) return;
    point = { x: event.clientX, y: event.clientY };
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      const bounds = card.getBoundingClientRect();
      const x = point.x - bounds.left, y = point.y - bounds.top;
      const distance = Math.hypot(Math.max(0, -x, x - bounds.width), Math.max(0, -y, y - bounds.height));
      const proximity = Math.max(0, 1 - distance / 64);
      card.style.setProperty('--edge-x', x + 'px');
      card.style.setProperty('--edge-y', y + 'px');
      card.style.setProperty('--edge-light', String(proximity * proximity));
    });
  });
  document.documentElement.addEventListener('pointerleave', clear);
  window.addEventListener('blur', clear);
  document.addEventListener('scroll', clear, true);
  pointer.addEventListener('change', clear);
})();
