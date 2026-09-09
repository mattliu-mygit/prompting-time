// Synthetic chat=tools, 100% scale, with operation details initially closed.
export function checkToolOperations() {
  const required = (selector: string) => {
    const element = document.querySelector<HTMLElement>(selector);
    if (!element) throw new Error(`Missing ${selector}`);
    return element;
  };
  const row = (sequence: number) => required(`[data-timeline-id="conversation-0-operation-${sequence}"]`);
  const running = row(5);
  const failed = row(6);
  const message = getComputedStyle(required(".message-markdown"));
  const visibleButtons = [running, failed].flatMap(element => [...element.querySelectorAll<HTMLButtonElement>("button")]);
  const result = {
    runningHeight: running.getBoundingClientRect().height,
    failedHeight: failed.getBoundingClientRect().height,
    runningTitle: running.textContent?.includes("Run cargo test"),
    failedTitle: failed.textContent?.includes("Run synthetic-check"),
    messageFont: message.fontSize,
    messageLineHeight: message.lineHeight,
    targets: visibleButtons.map(element => ({ width: element.getBoundingClientRect().width, height: element.getBoundingClientRect().height })),
    horizontalOverflow: document.documentElement.scrollWidth > innerWidth,
    verticalOverflow: document.documentElement.scrollHeight > innerHeight,
  };
  const failures: string[] = [];
  if (!result.runningTitle || !result.failedTitle) failures.push("running and failed operations must remain directly visible");
  if (result.runningHeight > 56 || result.failedHeight > 56) failures.push("closed operation rows are too tall");
  if (result.messageFont !== "17px" || result.messageLineHeight !== "27.2px") failures.push("message readability changed");
  if (!visibleButtons.length || result.targets.some(({ width, height }) => width < 31.5 || height < 31.5)) failures.push("detail controls need 32px targets");
  if (result.horizontalOverflow || result.verticalOverflow) failures.push("layout escapes viewport");
  if (failures.length) throw new Error(JSON.stringify({ failures, result }));
  return result;
}
