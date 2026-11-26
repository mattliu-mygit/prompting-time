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
  const message = getComputedStyle(required(".timeline-message .pt-markdown"));
  const visibleButtons = [running, failed].flatMap(element => [...element.querySelectorAll<HTMLButtonElement>("button")]);
  // A control is a 30px target, Sigil's small control height, or an inline text button such as
  // Show details: 12px text or larger in a summary line at least 30px high.
  const usable = (element: HTMLButtonElement) => {
    const box = element.getBoundingClientRect();
    if (box.width >= 29.5 && box.height >= 29.5) return true;
    const line = element.closest<HTMLElement>(".tool-operation-summary");
    return element.matches(".disclosure-link") && Number.parseFloat(getComputedStyle(element).fontSize) >= 12
      && line !== null && line.getBoundingClientRect().height >= 29.5;
  };
  const result = {
    runningHeight: running.getBoundingClientRect().height,
    failedHeight: failed.getBoundingClientRect().height,
    runningTitle: running.textContent?.includes("Run cargo test"),
    failedTitle: failed.textContent?.includes("Run synthetic-check"),
    messageFont: message.fontSize,
    messageLineHeight: message.lineHeight,
    targets: visibleButtons.map(element => ({ width: element.getBoundingClientRect().width, height: element.getBoundingClientRect().height, usable: usable(element) })),
    horizontalOverflow: document.documentElement.scrollWidth > innerWidth,
    verticalOverflow: document.documentElement.scrollHeight > innerHeight,
  };
  const failures: string[] = [];
  if (!result.runningTitle || !result.failedTitle) failures.push("running and failed operations must remain directly visible");
  if (result.runningHeight > 56 || result.failedHeight > 56) failures.push("closed operation rows are too tall");
  if (result.messageFont !== "16px" || result.messageLineHeight !== "26px") failures.push("message readability changed");
  if (!visibleButtons.length || result.targets.some(({ usable }) => !usable)) failures.push("detail controls need 30px targets or a 30px line");
  if (result.horizontalOverflow || result.verticalOverflow) failures.push("layout escapes viewport");
  if (failures.length) throw new Error(JSON.stringify({ failures, result }));
  return result;
}
