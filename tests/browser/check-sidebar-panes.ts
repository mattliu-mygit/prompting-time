// Run in real Chromium through Playwright CLI; jsdom cannot measure these bounds. Checks All work's
// sections as panes in whatever sections are open: every heading on screen, the stack of folded
// ones at the bottom, and the open ones sharing the space above it, each scrolling on its own.
export function checkSidebarPanes() {
  const all = document.querySelector<HTMLElement>(".all-work");
  if (!all) throw new Error("Missing .all-work");
  const list = all.getBoundingClientRect();
  const open = all.querySelector<HTMLElement>(".work-open");
  const stack = all.querySelector<HTMLElement>(".work-folded")?.getBoundingClientRect() ?? null;
  const headings = [...all.querySelectorAll<HTMLElement>(".work-group-toggle")].map((heading) => {
    const box = heading.getBoundingClientRect();
    return { name: heading.textContent ?? "", top: box.top, bottom: box.bottom };
  });
  const sections = [...all.querySelectorAll<HTMLElement>(".work-open > .work-section")].map((section) => {
    const body = section.querySelector<HTMLElement>(".work-section-body")!;
    const previous = body.scrollTop;
    body.scrollTop = 0;
    body.scrollTop = 50;
    const scrolled = body.scrollTop;
    body.scrollTop = previous;
    // How tall the rows are, from the body's top to its last child's bottom and its padding.
    const top = body.getBoundingClientRect().top;
    const rowsHeight = (body.lastElementChild?.getBoundingClientRect().bottom ?? top) - top + parseFloat(getComputedStyle(body).paddingBottom);
    return {
      name: section.querySelector(".work-group-label")?.textContent ?? "",
      height: section.getBoundingClientRect().height,
      bodyHeight: body.clientHeight,
      contentHeight: body.scrollHeight,
      rowsHeight,
      scrolled,
    };
  });
  const measurements = {
    list: { top: list.top, bottom: list.bottom },
    open: open ? open.getBoundingClientRect().height : 0,
    stack: stack && { top: stack.top, bottom: stack.bottom },
    headings,
    sections,
  };

  const failures: string[] = [];
  for (const heading of headings) {
    if (heading.top < list.top - 1 || heading.bottom > list.bottom + 1) failures.push(`the heading "${heading.name}" must stay on screen`);
  }
  if (stack && Math.abs(stack.bottom - list.bottom) > 1) failures.push("the folded sections must stack at the bottom");
  const overflowing = sections.filter((section) => section.contentHeight > section.bodyHeight + 1);
  for (const section of overflowing) {
    if (section.scrolled <= 0) failures.push(`"${section.name}" must scroll on its own`);
  }
  for (const section of sections.filter((item) => !overflowing.includes(item))) {
    if (section.bodyHeight - section.rowsHeight > 2) failures.push(`"${section.name}" must take only the space its rows need`);
  }
  if (overflowing.length > 0) {
    // A section whose rows need more than its share gets the share; one that needs less takes only what it needs.
    const used = sections.reduce((sum, section) => sum + section.height, 0);
    if (Math.abs(used - measurements.open) > 2) failures.push("the open sections must fill the space above the stack");
    const shares = overflowing.map((section) => section.height);
    if (Math.max(...shares) - Math.min(...shares) > 2) failures.push("the open sections that scroll must share the space evenly");
  }
  if (failures.length) throw new Error(JSON.stringify({ failures, measurements }));
  return measurements;
}
