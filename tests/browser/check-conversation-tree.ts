// Real browser geometry for the invented root / child / grandchild fixture.
// Expand both ancestors before checking; semantic depth alone is insufficient.
export function checkConversationTreeGeometry() {
  const levels = [1, 2, 3].map((level) => {
    const item = document.querySelector<HTMLElement>(`[role="treeitem"][aria-level="${level}"]`);
    const label = item?.querySelector<HTMLElement>(".tree-label");
    if (!item || !label) throw new Error(`Missing visible conversation at depth ${level}`);
    return {
      level,
      label: label.textContent,
      x: label.getBoundingClientRect().x,
      height: item.querySelector(".tree-row")?.getBoundingClientRect().height ?? 0,
    };
  });
  const failures: string[] = [];
  if (!levels.every((row, index) => index === 0 || row.x > levels[index - 1].x)) {
    failures.push("Each generation must have visibly greater indentation");
  }
  if (levels.some(({ height }) => height < 31.5)) failures.push("Conversation rows must retain 32px targets");
  if (document.documentElement.scrollWidth > innerWidth + 1 || document.documentElement.scrollHeight > innerHeight + 1) {
    failures.push("Layout must remain inside the viewport");
  }
  if (failures.length) throw new Error(JSON.stringify({ failures, levels }));
  return levels;
}

// Call after opening a known fixture conversation through its sidebar row.
export function checkSelectedConversation(title: string, ownMarker: string, otherMarkers: string[]) {
  const heading = document.querySelector("h1")?.textContent;
  const transcript = document.querySelector(".timeline-scroll")?.textContent ?? "";
  if (heading !== title) throw new Error(`Expected selected title ${title}, got ${heading}`);
  if (!transcript.includes(ownMarker)) throw new Error("Selected conversation's captured activity is absent");
  if (otherMarkers.some((marker) => transcript.includes(marker))) {
    throw new Error("Selected conversation contains another conversation's activity");
  }
  return { heading, ownActivityVisible: true };
}
