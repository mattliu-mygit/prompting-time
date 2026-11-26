// Run in real Chromium through Playwright CLI; jsdom cannot measure these bounds. Checks an open
// review's walkthrough and its quiet surroundings: All work folded to its rail, the review's
// controls in the window's one header row, the navigator open, the column centered beside it, the
// verdict first, and every control a 30px target.
export async function checkReviewWalkthrough() {
  const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
  const row = [...document.querySelectorAll<HTMLElement>('[role="treeitem"]')].find((item) => item.textContent?.includes("add a retry policy for uploads"));
  if (!row) throw new Error("Missing the row of widgets#101; open ?agents=reviews&work=1");
  row.click();
  for (let tries = 0; tries < 40 && !document.querySelector(".walkthrough"); tries += 1) await wait(100);
  const walkthrough = document.querySelector<HTMLElement>(".walkthrough");
  if (!walkthrough) throw new Error("Missing .walkthrough");
  const box = (selector: string) => document.querySelector<HTMLElement>(selector)?.getBoundingClientRect() ?? null;
  const toolbar = box(".app-toolbar")!;
  const rail = box("#work-pane")!;
  const scroller = box(".walk-scroll")!;
  const column = box(".walk-column")!;
  const nav = box(".walk-nav")!;
  const first = document.querySelector(".walk-column > [data-part]")?.querySelector(".verdict-banner");
  const controls = [...document.querySelectorAll<HTMLElement>(".toolbar-review button, .since-notice button, .walk-nav button, .decide-corner button, .walk-places button, .walk-layer-end button")]
    .filter((element) => element.getClientRects().length > 0);
  const small = controls.filter((element) => element.getBoundingClientRect().height < 29.5)
    .map((element) => `${element.className || element.tagName} "${element.textContent?.trim().slice(0, 30)}" ${element.getBoundingClientRect().height.toFixed(1)}px`);
  const label = document.querySelector<HTMLElement>(".work-rail-label");
  const result = {
    toolbarHeight: toolbar.height,
    railWidth: rail.width,
    railLabel: label?.textContent ?? null,
    railWritingMode: label ? getComputedStyle(label).writingMode : null,
    navWidth: nav.width,
    // How far the column's center is from the center of the space beside the navigator.
    columnOffset: Math.abs(column.left + column.width / 2 - (scroller.left + scroller.width / 2)),
    reviewControlsInHeader: Boolean(document.querySelector('.app-toolbar .toolbar-review [aria-label="More review actions"]')),
    verdictFirst: Boolean(first),
    small,
  };
  const failures: string[] = [];
  if (result.toolbarHeight < 48 || result.toolbarHeight > 56) failures.push("the header must stay one 48–56px row");
  if (!result.reviewControlsInHeader) failures.push("the review's controls must sit in the header row");
  if (result.railWidth > 48) failures.push("All work must fold to its rail while a review is open");
  if (!/^All work · \d+ needs? you$/.test(result.railLabel ?? "") || result.railWritingMode !== "vertical-rl") failures.push("the rail must say All work and how many need you, on its side");
  if (result.navWidth < 200) failures.push("the walkthrough's navigator must start open");
  if (result.columnOffset > 2) failures.push("the walkthrough's column must be centered beside the navigator");
  if (!result.verdictFirst) failures.push("the verdict banner must come first");
  if (small.length > 0) failures.push("the review's controls must be 30px targets");
  if (failures.length) throw new Error(JSON.stringify({ failures, result }));
  return result;
}
