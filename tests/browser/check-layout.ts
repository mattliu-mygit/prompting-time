// Run in real Chromium through Playwright CLI; jsdom cannot measure these bounds.
export function checkLayout() {
  const element = (selector: string) => {
    const result = document.querySelector<HTMLElement>(selector);
    if (!result) throw new Error(`Missing ${selector}`);
    return result;
  };
  const shell = element(".app-shell").getBoundingClientRect();
  const composer = element(".composer").getBoundingClientRect();
  const panes = [".timeline-scroll", ".sidebar-pane", ".inspector-pane"].map((selector) => {
    const pane = element(selector);
    const previous = pane.scrollTop;
    pane.scrollTop = 0;
    pane.scrollTop = 50;
    const result = { selector, height: pane.clientHeight, contentHeight: pane.scrollHeight, scrolled: pane.scrollTop };
    pane.scrollTop = previous;
    return result;
  });
  const measurements = {
    viewport: innerHeight,
    documentHeight: document.documentElement.scrollHeight,
    shellHeight: shell.height,
    composerTop: composer.top,
    composerBottom: composer.bottom,
    panes,
  };
  const failures = [];
  if (Math.abs(shell.height - innerHeight) > 1) failures.push("shell must fit viewport");
  if (document.documentElement.scrollHeight > innerHeight + 1) failures.push("document must not scroll vertically");
  if (composer.top < 0 || composer.bottom > innerHeight + 1) failures.push("composer must remain visible");
  for (const pane of panes) {
    if (pane.height <= 0 || pane.contentHeight <= pane.height || pane.scrolled <= 0) {
      failures.push(`${pane.selector} must scroll internally`);
    }
  }
  if (failures.length) throw new Error(JSON.stringify({ failures, measurements }));
  return measurements;
}
