// Run on chat=density at 100% scale, with no copy feedback currently open.
export function measureMessageDensity() {
  const required = (selector: string) => {
    const element = document.querySelector<HTMLElement>(selector);
    if (!element) throw new Error(`Missing ${selector}`);
    return element;
  };
  const user = required(".timeline-message.user");
  const assistant = required(".timeline-message.assistant");
  const lifecycle = required(".timeline-activity.lifecycle");
  const copy = required(".timeline-message.user button");
  return {
    userHeight: user.getBoundingClientRect().height,
    assistantHeight: assistant.getBoundingClientRect().height,
    lifecycleHeight: lifecycle.getBoundingClientRect().height,
    transcriptHeight: required(".timeline-list").getBoundingClientRect().height,
    messageFont: getComputedStyle(required(".message-markdown")).fontSize,
    messageLineHeight: getComputedStyle(required(".message-markdown")).lineHeight,
    copyWidth: copy.getBoundingClientRect().width,
    copyHeight: copy.getBoundingClientRect().height,
    copyInHeader: user.querySelector("header")?.contains(copy) ?? false,
    overflow: document.documentElement.scrollWidth > innerWidth || document.documentElement.scrollHeight > innerHeight,
  };
}

export function checkMessageDensity() {
  const result = measureMessageDensity();
  const failures: string[] = [];
  if (result.userHeight > 84) failures.push("short user message too tall");
  if (result.assistantHeight > 76) failures.push("short assistant message too tall");
  if (result.lifecycleHeight > 28) failures.push("ordinary lifecycle needs one compact line");
  if (result.messageFont !== "17px" || result.messageLineHeight !== "27.2px") failures.push("message readability changed");
  if (result.copyWidth < 31.5 || result.copyHeight < 31.5 || !result.copyInHeader) failures.push("copy must use an accessible header target");
  if (result.overflow) failures.push("layout escapes viewport");
  if (failures.length) throw new Error(JSON.stringify({ failures, result }));
  return result;
}
