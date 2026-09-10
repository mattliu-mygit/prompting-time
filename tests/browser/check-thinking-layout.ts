// Run against the invented composer fixture after the Thinking menu is visible.
export function checkThinkingLayout() {
  const composer = document.querySelector<HTMLElement>(".composer");
  if (!composer) throw new Error("Missing composer");
  const select = [...composer.querySelectorAll<HTMLSelectElement>("select")]
    .find(element => element.getAttribute("aria-label") === "Thinking"
      || (element.labels && [...element.labels].some(label => label.textContent?.includes("Thinking"))));
  if (!select) throw new Error("Missing accessible Thinking select");
  const targets = [...composer.querySelectorAll<HTMLElement>(".composer-actions select, .composer-actions button")]
    .filter(element => element.getBoundingClientRect().width > 0);
  const bounds = composer.getBoundingClientRect();
  const outside = targets.filter(element => {
    const rect = element.getBoundingClientRect();
    return rect.left < bounds.left - 1 || rect.right > bounds.right + 1
      || rect.top < bounds.top - 1 || rect.bottom > bounds.bottom + 1;
  });
  const overlaps = targets.flatMap((element, index) => targets.slice(index + 1).filter(other => {
    const first = element.getBoundingClientRect();
    const second = other.getBoundingClientRect();
    return Math.min(first.right, second.right) - Math.max(first.left, second.left) > 1
      && Math.min(first.bottom, second.bottom) - Math.max(first.top, second.top) > 1;
  }));
  const windowOverflow = document.documentElement.scrollWidth > innerWidth
    || document.documentElement.scrollHeight > innerHeight || bounds.bottom > innerHeight + 1;
  if (outside.length || overlaps.length || windowOverflow) {
    throw new Error(JSON.stringify({ outside: outside.length, overlaps: overlaps.length, windowOverflow }));
  }
  return {
    viewport: `${innerWidth}×${innerHeight}`,
    composerHeight: bounds.height,
    controls: targets.length,
    thinkingOptions: [...select.options].map(option => option.text),
  };
}
