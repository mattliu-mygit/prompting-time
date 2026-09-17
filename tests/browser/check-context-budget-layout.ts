// Run on the invented composer fixture; this measures real layout, not native settings.
export function checkContextBudgetLayout() {
  const composer = document.querySelector<HTMLElement>(".composer");
  if (!composer) throw new Error("Missing composer");
  const select = [...composer.querySelectorAll<HTMLSelectElement>("select")]
    .find(element => element.getAttribute("aria-label") === "Context budget"
      || [...(element.labels ?? [])].some(label => label.textContent?.includes("Context budget")));
  if (!select) throw new Error("Missing accessible Context budget select");
  const options = [...select.options].map(option => option.textContent?.trim());
  for (const expected of ["200k", "300k", "400k", "500k", "Provider default"]) {
    if (!options.includes(expected)) throw new Error(`Missing Context budget option: ${expected}`);
  }
  const controls = [...composer.querySelectorAll<HTMLElement>("select, button")]
    .filter(element => element.getBoundingClientRect().width > 0);
  const bounds = composer.getBoundingClientRect();
  const outside = controls.filter(element => {
    const rect = element.getBoundingClientRect();
    return rect.left < bounds.left - 1 || rect.right > bounds.right + 1
      || rect.top < bounds.top - 1 || rect.bottom > bounds.bottom + 1;
  });
  const overlaps = controls.flatMap((element, index) => controls.slice(index + 1).filter(other => {
    const a = element.getBoundingClientRect();
    const b = other.getBoundingClientRect();
    return Math.min(a.right, b.right) - Math.max(a.left, b.left) > 1
      && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 1;
  }));
  const windowOverflow = document.documentElement.scrollWidth > innerWidth
    || document.documentElement.scrollHeight > innerHeight || bounds.bottom > innerHeight + 1;
  if (outside.length || overlaps.length || windowOverflow) {
    throw new Error(JSON.stringify({ outside: outside.length, overlaps: overlaps.length, windowOverflow }));
  }
  return { viewport: `${innerWidth}×${innerHeight}`, composerHeight: bounds.height, options };
}
