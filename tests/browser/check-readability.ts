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
  const assistant = required(".timeline-message.assistant .pt-markdown");
  const row = required('[role="treeitem"]');
  const composer = required(".composer").getBoundingClientRect();
  const inspector = document.querySelector<HTMLElement>(".inspector-pane");
  const labelWidths = [...document.querySelectorAll<HTMLElement>('[role="treeitem"] .pt-list-row-title')].slice(0, 3).map((label) => label.clientWidth);
  const controls = [...document.querySelectorAll<HTMLElement>("button, select, textarea, input:not([type=checkbox]):not([type=radio])")].filter(visible);
  // Sizes from Sigil's default preset (@sigil-ui/presets): 12px (size-xs) is the smallest text,
  // 14px (size-sm) is a button's or field's, 13px is code's, and 30px (controls.height-sm) is a
  // control's height.
  const smallText = [...document.querySelectorAll<HTMLElement>("body *")]
    .filter((element) => visible(element) && [...element.childNodes].some((node) => node.nodeType === Node.TEXT_NODE && node.textContent?.trim()) && px(element) < 12)
    .map((element) => ({ tag: element.tagName, className: element.className, size: px(element) }));
  // One kind of control is smaller and sits inside a larger target: a row's expand and remove
  // buttons, in a tree row at least 36px high.
  const insideLargerTarget = (element: HTMLElement) =>
    element.matches(".work-disclosure, .work-remove") && (element.closest<HTMLElement>('[role="treeitem"]')?.getBoundingClientRect().height ?? 0) >= 35.5;
  const smallTargets = controls.filter((element) => element.getBoundingClientRect().height < 29.5 && !insideLargerTarget(element))
    .map((element) => ({ tag: element.tagName, className: element.className, height: element.getBoundingClientRect().height }));
  const smallControlText = controls.filter((element) => px(element) < 12)
    .map((element) => ({ tag: element.tagName, className: element.className, size: px(element) }));
  const offSizeFields = [...document.querySelectorAll<HTMLElement>(".pt-button, .pt-input")].filter((element) => visible(element) && px(element) !== 14)
    .map((element) => ({ tag: element.tagName, className: element.className, size: px(element) }));
  const measurements = {
    inputSize: px(input), replySize: px(assistant), sidebarSize: px(required('[role="treeitem"] .pt-list-row-title')), codeSize: px(required(".pt-code-block pre")),
    inputColor: getComputedStyle(input).color, foregroundColor: getComputedStyle(document.body).color,
    inputLineHeight: getComputedStyle(input).lineHeight, replyLineHeight: getComputedStyle(assistant).lineHeight,
    labelWidths, inspectorOverflow: inspector ? inspector.scrollWidth > inspector.clientWidth : false,
    rowHeight: row.getBoundingClientRect().height, inspectorOpen: inspector !== null,
    composerBottom: composer.bottom, viewportHeight: innerHeight,
    horizontalOverflow: document.documentElement.scrollWidth > innerWidth,
    verticalOverflow: document.documentElement.scrollHeight > innerHeight,
    smallText, smallTargets, smallControlText, offSizeFields,
  };
  const failures = [];
  if (measurements.inputSize !== 16 || measurements.replySize !== 16) failures.push("chat and input must be 16px");
  if (measurements.sidebarSize !== 14) failures.push("sidebar labels must be 14px");
  if (labelWidths.some((width) => width < 120)) failures.push("sidebar names are crowded by metadata");
  if (measurements.inspectorOverflow) failures.push("inspector content overflows horizontally");
  if (measurements.inputLineHeight !== "26px" || measurements.replyLineHeight !== "26px") failures.push("chat and input line height must be 1.625");
  if (measurements.inputColor !== measurements.foregroundColor) failures.push("typed text must use normal foreground");
  if (measurements.rowHeight < 36) failures.push("tree rows must be at least 36px");
  if (measurements.inspectorOpen !== inspectorOpen) failures.push("unexpected inspector visibility");
  if (smallText.length) failures.push("visible text below 12px");
  if (smallTargets.length) failures.push("controls below 30px high");
  if (smallControlText.length) failures.push("control text below 12px");
  if (offSizeFields.length) failures.push("buttons and fields must be 14px");
  if (measurements.codeSize < 13) failures.push("code below 13px");
  if (composer.top < 0 || composer.bottom > innerHeight + 1 || measurements.horizontalOverflow || measurements.verticalOverflow) failures.push("content escapes viewport");
  if (failures.length) throw new Error(JSON.stringify({ failures, measurements }));
  return measurements;
}
