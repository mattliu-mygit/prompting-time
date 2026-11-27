// Run in real Chromium through Playwright CLI; jsdom computes no styles. Opens widgets#103, whose run
// of the tests couldn't measure, then gadgets#68's Changes no test would catch in full, and steps to
// its first file with j. Checks what only a browser shows: #103 says why in one quiet line; #68's
// marked lines no test ran are orange in the gutter under the words that name them, and a line
// that ran isn't; nothing moves after j; no transition runs longer than 200 ms; all text is 12px or
// more; every control is a 30px target; and nothing reaches past the window's right edge.
export async function checkReviewTestGaps() {
  const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
  const until = async <T>(find: () => T | null | undefined) => {
    for (let tries = 0; tries < 50 && !find(); tries += 1) await wait(100);
    return find() ?? null;
  };
  const open = (title: string) => {
    const row = [...document.querySelectorAll<HTMLElement>('[role="treeitem"]')].find((item) => item.textContent?.includes(title));
    if (!row) throw new Error(`Missing the row of "${title}"; open ?agents=reviews&work=1`);
    row.click();
  };

  open("cap the upload retry delay");
  const quiet = await until(() => document.querySelector<HTMLElement>(".walk-view-quiet"));
  const quietText = quiet?.textContent ?? null;
  open("validate project names before creating buckets");
  const button = await until(() => [...document.querySelectorAll<HTMLButtonElement>(".walk-view-summary button")].find((item) => item.textContent === "Open all lines"));
  if (!button) throw new Error("Missing Open all lines in gadgets#68's walkthrough");
  const placeLine = document.querySelector(".walk-places li .place-gaps")?.textContent ?? null;
  button.click();
  const container = await until(() => document.querySelector(".gaps-file diffs-container"));
  if (!container) throw new Error("Missing the diff of create.go in Changes no test would catch");
  await wait(800);
  document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "j", bubbles: true }));
  await wait(50);

  const view = document.querySelector<HTMLElement>(".full-view")!;
  const file = document.querySelector<HTMLElement>(".gaps-file")!;
  const warning = getComputedStyle(document.documentElement).getPropertyValue("--s-warning").trim();
  const probe = document.body.appendChild(document.createElement("span"));
  probe.style.color = warning;
  const orange = getComputedStyle(probe).color;
  probe.remove();
  const gutter = (line: number) => {
    const cell = container.shadowRoot?.querySelector<HTMLElement>(`[data-column-number="${line}"]:not([data-line-type="change-deletion"])`);
    return cell ? { color: getComputedStyle(cell).color, cursor: getComputedStyle(cell).cursor } : null;
  };
  const seconds = (value: string) => Math.max(...value.split(",").map((part) => (part.trim().endsWith("ms") ? parseFloat(part) / 1000 : parseFloat(part))));
  const elements = [view, ...view.querySelectorAll<Element>("*")];
  const slow = elements.filter((element) => seconds(getComputedStyle(element).transitionDuration) > 0.2 || seconds(getComputedStyle(element).animationDuration) > 0.2)
    .map((element) => `${element.tagName.toLowerCase()}.${element.getAttribute("class") ?? ""}`);
  const small = elements.filter((element) => [...element.childNodes].some((child) => child.nodeType === Node.TEXT_NODE && child.textContent?.trim()) && parseFloat(getComputedStyle(element).fontSize) < 12)
    .map((element) => `${element.tagName.toLowerCase()} "${element.textContent?.trim().slice(0, 30)}" ${getComputedStyle(element).fontSize}`);
  const tiny = [...view.querySelectorAll<Element>("button")].filter((element) => element.getClientRects().length > 0 && element.getBoundingClientRect().height < 29.5)
    .map((element) => `${element.getAttribute("class") ?? element.tagName} "${element.textContent?.trim().slice(0, 30)}"`);
  const result = {
    quietText,
    placeLine,
    words: file.querySelector("p")?.textContent ?? null,
    notRun: gutter(19),
    ran: gutter(23),
    orange,
    current: file.hasAttribute("data-current"),
    running: document.getAnimations().length,
    slow,
    small,
    tiny,
    overflow: Math.max(view.scrollWidth - view.clientWidth, document.documentElement.scrollWidth - innerWidth),
  };
  const failures: string[] = [];
  if (!result.quietText?.includes("Not measured: Whooper measures Go files only so far")) failures.push("widgets#103 must say Not measured and why in one quiet line");
  if (result.placeLine !== "No test ran 3 changed lines here: 19–21Measured: no test ran these lines · go test -coverprofile ./internal/projects/...") failures.push("gadgets#68's first place must name its lines no test ran, labeled measured");
  if (result.words !== "No test ran 4 changed lines: 19–22") failures.push("create.go must name its lines no test ran above its diff");
  if (result.notRun?.color !== orange || result.ran?.color === orange) failures.push("only lines no test ran may be orange in the gutter");
  if (result.notRun?.cursor === "pointer") failures.push("a mark nothing handles a click on must not show a pointer");
  if (!result.current) failures.push("j must make the first file current");
  if (result.running > 0) failures.push("nothing may move after j");
  if (slow.length > 0) failures.push("no transition may run longer than 200 ms");
  if (small.length > 0) failures.push("text must be 12px or more");
  if (tiny.length > 0) failures.push("controls must be 30px targets");
  if (result.overflow > 1) failures.push("nothing may reach past the window's right edge");
  if (failures.length) throw new Error(JSON.stringify({ failures, result }));
  return result;
}
