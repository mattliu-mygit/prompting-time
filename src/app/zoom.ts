const preferenceKey = "prompting-time.zoom";

function readZoom(): number {
  try {
    const value = Number(window.localStorage.getItem(preferenceKey));
    if (Number.isFinite(value) && value >= 0.75 && value <= 2) return value;
  } catch {
    // The display preference is optional when storage is unavailable.
  }
  return 1;
}

function reportZoomFailure(error: unknown): void {
  console.warn("Could not apply app zoom.", error);
}

export function installAppZoom(setZoom: (factor: number) => Promise<void>): () => void {
  let zoom = 1;
  let disposed = false;

  async function applyZoom(next: number): Promise<void> {
    await setZoom(next);
    zoom = next;
    if (disposed) return;
    try {
      window.localStorage.setItem(preferenceKey, String(zoom));
    } catch {
      // Storage failure must not undo a successful native change.
    }
  }

  // Each shortcut uses the last successful scale, after earlier requests finish.
  let pending = applyZoom(readZoom()).catch(reportZoomFailure);
  function onKeyDown(event: KeyboardEvent): void {
    if (!event.metaKey || event.ctrlKey || event.altKey || event.isComposing || event.keyCode === 229) return;
    const key = event.key;
    if (key !== "=" && key !== "+" && key !== "-" && key !== "0") return;
    event.preventDefault();
    pending = pending.then(async () => {
      if (disposed) return;
      const next = key === "0" ? 1 : Math.min(2, Math.max(0.75, zoom + (key === "-" ? -0.25 : 0.25)));
      if (next !== zoom) await applyZoom(next);
    }).catch(reportZoomFailure);
  }

  window.addEventListener("keydown", onKeyDown, { capture: true });
  return () => {
    disposed = true;
    window.removeEventListener("keydown", onKeyDown, { capture: true });
  };
}
