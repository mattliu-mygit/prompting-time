// Browser geometry, not jsdom: run on an idle root conversation at 100% zoom.
export function checkCompactLayout() {
  function required(selector: string) {
    const element = document.querySelector<HTMLElement>(selector);
    if (!element) throw new Error(`Missing ${selector}`);
    return element;
  }
  const header = required(".app-toolbar").getBoundingClientRect();
  const timeline = required(".timeline-scroll").getBoundingClientRect();
  const composer = required(".composer").getBoundingClientRect();
  const input = getComputedStyle(required(".message-field textarea"));
  const controls = [...required(".app-toolbar").querySelectorAll<HTMLElement>("button")];
  const measurements = {
    headerHeight: header.height,
    timelineTop: timeline.top,
    timelineHeight: timeline.height,
    composerHeight: composer.height,
    composerBottom: composer.bottom,
    inputFont: input.fontSize,
    horizontalOverflow: document.documentElement.scrollWidth > innerWidth,
    verticalOverflow: document.documentElement.scrollHeight > innerHeight,
    headerTargets: controls.map(element => element.getBoundingClientRect().height),
  };
  const failures: string[] = [];
  if (header.height < 48 || header.height > 56) failures.push("header must occupy one 48–56px row");
  if (timeline.top > 72) failures.push("root timeline starts too far below the header");
  if (composer.height > 144) failures.push("idle composer should have only field and footer");
  if (measurements.inputFont !== "17px") failures.push("input typography shrank");
  if (measurements.headerTargets.some(height => height < 31.5)) failures.push("header targets shrank below 32px");
  if (composer.bottom > innerHeight + 1 || measurements.horizontalOverflow || measurements.verticalOverflow) failures.push("layout escapes the window");
  if (failures.length) throw new Error(JSON.stringify({ failures, measurements }));
  return measurements;
}

export function checkComposerHelpLayout() {
  const help = document.querySelector<HTMLElement>(".composer-help-content");
  const toggle = document.querySelector<HTMLElement>(".composer-help");
  if (!help || toggle?.getAttribute("aria-expanded") !== "true" || help.hidden) {
    throw new Error("Open Composer help before checking its layout");
  }
  const footerControls = [...document.querySelectorAll<HTMLElement>(".composer-actions button, .composer-actions select")];
  const helpTop = help.getBoundingClientRect().top;
  const footerBottom = Math.max(...footerControls.map(element => element.getBoundingClientRect().bottom));
  if (helpTop < footerBottom) throw new Error(JSON.stringify({ helpTop, footerBottom }));
  return { helpTop, footerBottom };
}
