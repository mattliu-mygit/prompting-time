// Run in real Chromium through Playwright CLI; jsdom computes no styles. Opens gadgets#68's map of
// how the changed code connects, selects projects.ValidateName, and opens its diff; then opens
// gadgets#69's map, of a TypeScript change, and selects formatTokens. On each map it checks what only
// a browser shows: a link found by name search is dashed and the others solid, whether gopls or
// TypeScript found them; the legend and the selected node's links say "found by name search" and
// "found by gopls" or "found by TypeScript"; nothing in the view moves after the selection; no
// transition runs longer than 200 ms; all text is 12px or more; every control is a 30px target; and
// the map and its panel stay inside the window.
export async function checkReviewConnections() {
  const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
  const until = async <T>(find: () => T | null | undefined) => {
    for (let tries = 0; tries < 40 && !find(); tries += 1) await wait(100);
    return find() ?? null;
  };
  /** Opens the map in the walkthrough of the review in the row titled `title`, of run `run`, and selects its node `id`. */
  async function openMap(title: string, run: string, id: string) {
    const row = [...document.querySelectorAll<HTMLElement>('[role="treeitem"]')].find((item) => item.textContent?.includes(title));
    if (!row) throw new Error(`Missing the row of "${title}"; open ?agents=reviews&work=1`);
    row.click();
    // Found by the summary's heading, which names the run, so the last review's Open the map can't answer.
    const open = await until(() => document.getElementById(`connections-${run}`)?.closest(".walk-connections")?.querySelector<HTMLButtonElement>("button"));
    if (!open) throw new Error(`Missing Open the map in the walkthrough of "${title}"`);
    open.click();
    const node = await until(() => document.querySelector(`.map-node[data-node="${id}"]`));
    if (!node) throw new Error(`Missing ${id} on the map of "${title}"`);
    node.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  }
  /** What only a browser shows of the open map: the dashes of a link found by name search and of one `tool` found, the words of the legend and the selected node's links, and the layout. */
  function measure(tool: string) {
    const view = document.querySelector<HTMLElement>(".map-view")!;
    const dash = (selector: string) => {
      const found = view.querySelector(selector);
      return found ? getComputedStyle(found).strokeDasharray : null;
    };
    const seconds = (value: string) => Math.max(...value.split(",").map((part) => (part.trim().endsWith("ms") ? parseFloat(part) / 1000 : parseFloat(part))));
    const elements = [view, ...view.querySelectorAll<Element>("*")];
    const targets = [...view.querySelectorAll<Element>("button, .map-node")].filter((element) => element.getClientRects().length > 0);
    return {
      nameSearchDash: dash('.map-edge[data-found-by="name_search"]'),
      toolDash: dash(`.map-edge[data-found-by="${tool}"]`),
      legend: view.querySelector(".map-legend")?.textContent ?? "",
      links: [...view.querySelectorAll<HTMLElement>(".map-panel li[data-found-by]")].map((item) => ({ foundBy: item.dataset.foundBy ?? "", says: item.querySelector(".map-found")?.textContent ?? "" })),
      running: document.getAnimations().length,
      slow: elements.filter((element) => seconds(getComputedStyle(element).transitionDuration) > 0.2 || seconds(getComputedStyle(element).animationDuration) > 0.2)
        .map((element) => `${element.tagName.toLowerCase()}.${element.getAttribute("class") ?? ""}`),
      small: elements.filter((element) => [...element.childNodes].some((child) => child.nodeType === Node.TEXT_NODE && child.textContent?.trim()) && parseFloat(getComputedStyle(element).fontSize) < 12)
        .map((element) => `${element.tagName.toLowerCase()} "${element.textContent?.trim().slice(0, 30)}" ${getComputedStyle(element).fontSize}`),
      tiny: targets.filter((element) => element.getBoundingClientRect().height < 29.5)
        .map((element) => `${element.getAttribute("class") ?? element.tagName} "${element.textContent?.trim().slice(0, 30)}" ${element.getBoundingClientRect().height.toFixed(1)}px`),
      outside: [".map-canvas", ".map-panel", ".map-legend"].filter((selector) => {
        const box = document.querySelector(selector)?.getBoundingClientRect();
        return !box || box.right > innerWidth + 0.5 || box.bottom > innerHeight + 0.5;
      }),
    };
  }

  await openMap("validate project names before creating buckets", "fixture-run-68", "validate");
  (await until(() => document.querySelector<HTMLButtonElement>(".map-location-open")))?.click();
  await until(() => document.querySelector(".map-code diffs-container"));
  await wait(300);
  const go = measure("gopls");
  await openMap("show token counts in the trace sidebar", "fixture-run-69", "format");
  await wait(300);
  const typescript = measure("typescript");

  const failures: string[] = [];
  for (const [review, result, tool] of [["gadgets#68", go, "gopls"], ["gadgets#69", typescript, "TypeScript"]] as const) {
    const says = `found by ${tool}`;
    if (result.nameSearchDash === null || result.nameSearchDash === "none" || result.toolDash !== "none") failures.push(`${review}: a link found by name search must be dashed, and one ${says} solid`);
    if (!result.legend.includes(`calls or uses, ${says}`) || !result.legend.includes("found by name search")) failures.push(`${review}: the legend must say "${says}" and "found by name search"`);
    const searched = result.links.filter((link) => link.foundBy === "name_search");
    const others = result.links.filter((link) => link.foundBy !== "name_search");
    if (searched.length === 0 || others.length === 0 || !searched.every((link) => link.says.startsWith("found by name search, so ")) || !others.every((link) => link.says === says)) {
      failures.push(`${review}: the selected node's links must say "${says}" or "found by name search" with its warning`);
    }
    if (result.running > 0) failures.push(`${review}: nothing may move after a selection`);
    if (result.slow.length > 0) failures.push(`${review}: no transition may run longer than 200 ms`);
    if (result.small.length > 0) failures.push(`${review}: text must be 12px or more`);
    if (result.tiny.length > 0) failures.push(`${review}: controls must be 30px targets`);
    if (result.outside.length > 0) failures.push(`${review}: the map, its legend, and its panel must stay inside the window`);
  }
  if (failures.length) throw new Error(JSON.stringify({ failures, go, typescript }));
  return { go, typescript };
}
