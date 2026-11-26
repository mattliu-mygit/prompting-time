import { readPreference, savePreference } from "./preferences";

/** The sidebar's width in CSS pixels: dragging keeps it within these, and this device keeps it across launches. */
export const SIDEBAR_WIDTH = { min: 220, max: 520, initial: 300 } as const;
const WIDTH_PREFERENCE = "prompting-time.sidebar-width";
/** How far each arrow key moves the edge. */
const KEY_STEP = 16;

const clamp = (width: number) => Math.round(Math.min(SIDEBAR_WIDTH.max, Math.max(SIDEBAR_WIDTH.min, width)));

/** The width a past launch left, or the initial width. */
export function readSidebarWidth(): number {
  const saved = Number(readPreference(WIDTH_PREFERENCE));
  return Number.isFinite(saved) && saved > 0 ? clamp(saved) : SIDEBAR_WIDTH.initial;
}

type Props = { width: number; onResize(width: number): void };

/**
 * The sidebar's right edge: drag it, or focus it and press the left and right arrow keys, to resize
 * the sidebar within its limits. The width is saved when a drag ends and after each key.
 */
export function SidebarResizer(props: Props) {
  let start: { x: number; width: number } | null = null;
  // The width this edge last set, which Solid shows in props.width only after its next flush.
  let latest: number | null = null;
  function resize(width: number) {
    latest = clamp(width);
    props.onResize(latest);
  }
  const save = () => savePreference(WIDTH_PREFERENCE, String(latest ?? props.width));
  const end = () => {
    if (!start) return;
    start = null;
    document.body.classList.remove("sidebar-resizing");
    save();
  };
  return (
    <div
      class="sidebar-resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize All work"
      aria-valuemin={SIDEBAR_WIDTH.min}
      aria-valuemax={SIDEBAR_WIDTH.max}
      aria-valuenow={props.width}
      tabindex="0"
      title="Drag to resize"
      onPointerDown={(event: PointerEvent) => {
        event.preventDefault();
        start = { x: event.clientX, width: latest ?? props.width };
        (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);
        document.body.classList.add("sidebar-resizing");
      }}
      onPointerMove={(event: PointerEvent) => {
        if (start) resize(start.width + event.clientX - start.x);
      }}
      onPointerUp={end}
      onPointerCancel={end}
      onKeyDown={(event: KeyboardEvent) => {
        const step = event.key === "ArrowLeft" ? -KEY_STEP : event.key === "ArrowRight" ? KEY_STEP : 0;
        if (step === 0) return;
        event.preventDefault();
        resize((latest ?? props.width) + step);
        save();
      }}
    />
  );
}
