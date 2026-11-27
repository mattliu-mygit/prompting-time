// Run in real Chromium through Playwright CLI; jsdom computes no styles. Opens gadgets#68's map of
// how the changed code connects, selects projects.ValidateName, and opens its diff, then checks what
// only a browser shows: a link found by name search is dashed and the others solid, nothing in the
// view moves after the selection, no transition runs longer than 200 ms, all text is 12px or more,
// every control is a 30px target, and the map and its panel stay inside the window.
export async function checkReviewConnections() {
  const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
  const until = async (find: () => Element | null | undefined) => {
    for (let tries = 0; tries < 40 && !find(); tries += 1) await wait(100);
    return find() ?? null;
  };
  const row = [...document.querySelectorAll<HTMLElement>('[role="treeitem"]')].find((item) => item.textContent?.includes("validate project names before creating buckets"));
  if (!row) throw new Error("Missing the row of gadgets#68; open ?agents=reviews&work=1");
  row.click();
  const open = await until(() => [...document.querySelectorAll<HTMLButtonElement>(".walk-connections button")].find((button) => button.textContent === "Open the map"));
  if (!open) throw new Error("Missing Open the map in gadgets#68's walkthrough");
  (open as HTMLButtonElement).click();
  const node = await until(() => document.querySelector('.map-node[data-node="validate"]'));
  if (!node) throw new Error("Missing projects.ValidateName on the map");
  node.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  (await until(() => document.querySelector<HTMLButtonElement>(".map-location-open")) as HTMLButtonElement | null)?.click();
  await until(() => document.querySelector(".map-code diffs-container"));
  await wait(300);

  const view = document.querySelector<HTMLElement>(".map-view")!;
  const dash = (selector: string) => getComputedStyle(document.querySelector(selector)!).strokeDasharray;
  const seconds = (value: string) => Math.max(...value.split(",").map((part) => (part.trim().endsWith("ms") ? parseFloat(part) / 1000 : parseFloat(part))));
  const elements = [view, ...view.querySelectorAll<Element>("*")];
  const slow = elements.filter((element) => seconds(getComputedStyle(element).transitionDuration) > 0.2 || seconds(getComputedStyle(element).animationDuration) > 0.2)
    .map((element) => `${element.tagName.toLowerCase()}.${element.getAttribute("class") ?? ""}`);
  const small = elements.filter((element) => [...element.childNodes].some((child) => child.nodeType === Node.TEXT_NODE && child.textContent?.trim()) && parseFloat(getComputedStyle(element).fontSize) < 12)
    .map((element) => `${element.tagName.toLowerCase()} "${element.textContent?.trim().slice(0, 30)}" ${getComputedStyle(element).fontSize}`);
  const targets = [...view.querySelectorAll<Element>("button, .map-node")].filter((element) => element.getClientRects().length > 0);
  const tiny = targets.filter((element) => element.getBoundingClientRect().height < 29.5)
    .map((element) => `${element.getAttribute("class") ?? element.tagName} "${element.textContent?.trim().slice(0, 30)}" ${element.getBoundingClientRect().height.toFixed(1)}px`);
  const outside = [".map-canvas", ".map-panel", ".map-legend"].filter((selector) => {
    const box = document.querySelector(selector)?.getBoundingClientRect();
    return !box || box.right > innerWidth + 0.5 || box.bottom > innerHeight + 0.5;
  });
  const result = {
    nameSearchDash: dash('.map-edge[data-found-by="name_search"]'),
    goplsDash: dash('.map-edge[data-found-by="gopls"]'),
    legendSaysNameSearch: document.querySelector(".map-legend")?.textContent?.includes("found by name search") ?? false,
    linksSayNameSearch: [...document.querySelectorAll('.map-panel li[data-found-by="name_search"]')].every((item) => item.textContent?.includes("found by name search")),
    running: document.getAnimations().length,
    slow,
    small,
    tiny,
    outside,
  };
  const failures: string[] = [];
  if (result.nameSearchDash === "none" || result.goplsDash !== "none") failures.push("a link found by name search must be dashed, and one found by gopls solid");
  if (!result.legendSaysNameSearch || !result.linksSayNameSearch) failures.push('the legend and the links must say "found by name search"');
  if (result.running > 0) failures.push("nothing may move after a selection");
  if (slow.length > 0) failures.push("no transition may run longer than 200 ms");
  if (small.length > 0) failures.push("text must be 12px or more");
  if (tiny.length > 0) failures.push("controls must be 30px targets");
  if (outside.length > 0) failures.push("the map, its legend, and its panel must stay inside the window");
  if (failures.length) throw new Error(JSON.stringify({ failures, result }));
  return result;
}
