// Real-browser checks: computed typography and geometry are not modeled by jsdom.
export function checkReadability({ inspectorOpen = false } = {}) {
  const required = (selector: string) => {
    const element = document.querySelector<HTMLElement>(selector);
    if (!element) throw new Error(`Missing ${selector}`);
    return element;
  };
  const visible = (element: HTMLElement) => element.getClientRects().length > 0
    && getComputedStyle(element).visibility !== "hidden";
  const px = (element: HTMLElement) => Number.parseFloat(getComputedStyle(element).fontSize);
  const input = required(".message-field textarea");
  const assistant = required(".timeline-message.assistant .message-markdown");
  const row = required(".tree-row");
  const composer = required(".composer").getBoundingClientRect();
  const inspector = document.querySelector<HTMLElement>(".inspector-pane");
  const labelWidths = [...document.querySelectorAll<HTMLElement>(".tree-label")].slice(0, 3).map((label) => label.clientWidth);
  const controls = [...document.querySelectorAll<HTMLElement>("button, select, textarea, input:not([type=checkbox]):not([type=radio])")].filter(visible);
  const smallText = [...document.querySelectorAll<HTMLElement>("body *")]
    .filter((element) => visible(element) && [...element.childNodes].some((node) => node.nodeType === Node.TEXT_NODE && node.textContent?.trim()) && px(element) < 13)
    .map((element) => ({ tag: element.tagName, className: element.className, size: px(element) }));
  const smallTargets = controls.filter((element) => element.getBoundingClientRect().height < 31.5)
    .map((element) => ({ tag: element.tagName, className: element.className, height: element.getBoundingClientRect().height }));
  const smallControlText = controls.filter((element) => px(element) < 14)
    .map((element) => ({ tag: element.tagName, className: element.className, size: px(element) }));
  const measurements = {
    inputSize: px(input), replySize: px(assistant), sidebarSize: px(required(".tree-label")), codeSize: px(required(".code-block pre")),
    inputColor: getComputedStyle(input).color, foregroundColor: getComputedStyle(document.body).color,
    inputLineHeight: getComputedStyle(input).lineHeight, replyLineHeight: getComputedStyle(assistant).lineHeight,
    labelWidths, inspectorOverflow: inspector ? inspector.scrollWidth > inspector.clientWidth : false,
    rowHeight: row.getBoundingClientRect().height, inspectorOpen: inspector !== null,
    composerBottom: composer.bottom, viewportHeight: innerHeight,
    horizontalOverflow: document.documentElement.scrollWidth > innerWidth,
    verticalOverflow: document.documentElement.scrollHeight > innerHeight,
    smallText, smallTargets, smallControlText,
  };
  const failures = [];
  if (measurements.inputSize !== 17 || measurements.replySize !== 17) failures.push("chat and input must be 17px");
  if (measurements.sidebarSize !== 14) failures.push("sidebar labels must be 14px");
  if (labelWidths.some((width) => width < 120)) failures.push("sidebar names are crowded by metadata");
  if (measurements.inspectorOverflow) failures.push("inspector content overflows horizontally");
  if (measurements.inputLineHeight !== "27.2px" || measurements.replyLineHeight !== "27.2px") failures.push("chat and input line height must be 1.6");
  if (measurements.inputColor !== measurements.foregroundColor) failures.push("typed text must use normal foreground");
  if (measurements.rowHeight < 36) failures.push("tree rows must be at least 36px");
  if (measurements.inspectorOpen !== inspectorOpen) failures.push("unexpected inspector visibility");
  if (smallText.length) failures.push("visible text below 13px");
  if (smallTargets.length) failures.push("controls below 32px high");
  if (smallControlText.length) failures.push("control text below 14px");
  if (measurements.codeSize < 14) failures.push("code below 14px");
  if (composer.top < 0 || composer.bottom > innerHeight + 1 || measurements.horizontalOverflow || measurements.verticalOverflow) failures.push("content escapes viewport");
  if (failures.length) throw new Error(JSON.stringify({ failures, measurements }));
  return measurements;
}
