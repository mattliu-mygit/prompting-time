// Run in real Chromium through Playwright CLI; jsdom computes no styles. Opens gadgets#68's Example
// inputs, before and after, in full, and steps to the first input with j. Checks what only a browser
// shows: the walkthrough's line sits inside What this PR does; only "changed" is orange; the changed
// rows come first; nothing moves after j; no transition runs longer than 200 ms; all text is 12px or
// more; every control is a 30px target; and no table reaches past the window's right edge.
export async function checkReviewExamples() {
  const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
  const until = async <T>(find: () => T | null | undefined) => {
    for (let tries = 0; tries < 50 && !find(); tries += 1) await wait(100);
    return find() ?? null;
  };
  const row = [...document.querySelectorAll<HTMLElement>('[role="treeitem"]')].find((item) => item.textContent?.includes("validate project names before creating buckets"));
  if (!row) throw new Error("Missing the row of gadgets#68; open ?agents=reviews&work=1");
  row.click();
  const line = await until(() => document.querySelector<HTMLElement>(".walk-view-line"));
  if (!line) throw new Error("Missing the line of Example inputs, before and after, in gadgets#68's walkthrough");
  const insideWhat = Boolean(line.closest('[data-part="what"]'));
  const lineText = line.querySelector("p")?.textContent ?? null;
  [...line.querySelectorAll("button")].find((button) => button.textContent === "Open all inputs")?.click();
  const table = await until(() => document.querySelector<HTMLElement>(".examples-table"));
  if (!table) throw new Error("Missing the tables of Example inputs, before and after");
  document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "j", bubbles: true }));
  await wait(50);

  const view = document.querySelector<HTMLElement>(".full-view")!;
  const warning = getComputedStyle(document.documentElement).getPropertyValue("--s-warning").trim();
  const probe = document.body.appendChild(document.createElement("span"));
  probe.style.color = warning;
  const orange = getComputedStyle(probe).color;
  probe.remove();
  const words = [...view.querySelectorAll<HTMLElement>(".example-status")].map((element) => ({ word: element.textContent, orange: getComputedStyle(element).color === orange }));
  const seconds = (value: string) => Math.max(...value.split(",").map((part) => (part.trim().endsWith("ms") ? parseFloat(part) / 1000 : parseFloat(part))));
  const elements = [view, ...view.querySelectorAll<Element>("*")];
  const slow = elements.filter((element) => seconds(getComputedStyle(element).transitionDuration) > 0.2 || seconds(getComputedStyle(element).animationDuration) > 0.2)
    .map((element) => `${element.tagName.toLowerCase()}.${element.getAttribute("class") ?? ""}`);
  const small = elements.filter((element) => [...element.childNodes].some((child) => child.nodeType === Node.TEXT_NODE && child.textContent?.trim()) && parseFloat(getComputedStyle(element).fontSize) < 12)
    .map((element) => `${element.tagName.toLowerCase()} "${element.textContent?.trim().slice(0, 30)}" ${getComputedStyle(element).fontSize}`);
  const tiny = [...view.querySelectorAll<Element>("button")].filter((element) => element.getClientRects().length > 0 && element.getBoundingClientRect().height < 29.5)
    .map((element) => `${element.getAttribute("class") ?? element.tagName} "${element.textContent?.trim().slice(0, 30)}"`);
  const result = {
    insideWhat,
    lineText,
    firstRows: [...table.querySelectorAll("tbody tr")].slice(0, 3).map((item) => item.querySelector("td")?.textContent),
    words,
    current: table.querySelector('tbody tr[aria-current="true"] td')?.textContent ?? null,
    running: document.getAnimations().length,
    slow,
    small,
    tiny,
    overflow: Math.max(...[...view.querySelectorAll<HTMLElement>(".examples-table")].map((element) => element.getBoundingClientRect().right - innerWidth), document.documentElement.scrollWidth - innerWidth),
  };
  const failures: string[] = [];
  if (!result.insideWhat || result.lineText !== 'Example inputs, before and afterprojects.Create gives a different result for "Q3 Report", "a b", and "ab". Ran at base and head.') failures.push("the line must name the changed inputs inside What this PR does");
  if (result.firstRows.join() !== '"Q3 Report","a b","ab"') failures.push("the changed rows must come first");
  if (!words.every((item) => item.orange === (item.word === "changed")) || !words.some((item) => item.orange)) failures.push('only "changed" may be orange');
  if (result.current !== '"Q3 Report"') failures.push("j must make the first input current");
  if (result.running > 0) failures.push("nothing may move after j");
  if (slow.length > 0) failures.push("no transition may run longer than 200 ms");
  if (small.length > 0) failures.push("text must be 12px or more");
  if (tiny.length > 0) failures.push("controls must be 30px targets");
  if (result.overflow > 1) failures.push("no table may reach past the window's right edge");
  if (failures.length) throw new Error(JSON.stringify({ failures, result }));
  return result;
}
