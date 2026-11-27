import { createSignal } from "solid-js";
import type { PaletteGroup } from "@prompting-time/ui";
import { readPreference, savePreference } from "../../app/preferences";
import type { ShownReview } from "./shown";

/**
 * The views of a review beyond its walkthrough. Each adds a short summary where its question comes up
 * in the walkthrough, and only when the plugin printed data for it in this review. The Views menu
 * lists every view, saying when one has nothing for this review, except one marked `onlyWithData`,
 * which it lists only for a review that has its data.
 */
export type ViewId = "since" | "calls" | "co-change" | "connections" | "test-gaps" | "inputs";
export type ReviewView = { id: ViewId; name: string; description: string; onlyWithData?: boolean };
export const REVIEW_VIEWS: readonly ReviewView[] = [
  { id: "since", name: "Changes since you last looked", description: "New commits since the review, and what you last read." },
  { id: "inputs", name: "Example inputs, before and after", description: "What the old and new code give for the same inputs, from running both.", onlyWithData: true },
  { id: "test-gaps", name: "Changes no test would catch", description: "Changed lines no test ran, from a run of the tests.", onlyWithData: true },
  { id: "calls", name: "Calls before and after", description: "Old and new calls for one input, overlaid." },
  { id: "co-change", name: "Files that usually change with these", description: "Files that change with these in history, left alone here." },
  { id: "connections", name: "How the changed code connects", description: "Changed functions and types, and their callers.", onlyWithData: true },
];

/** The views that show in full in the walkthrough's place, opened from their summary. */
export type FullViewId = Extract<ViewId, "connections" | "test-gaps" | "inputs">;

export const viewName = (id: ViewId) => REVIEW_VIEWS.find((view) => view.id === id)!.name;

/** Whether the review has something for the view to show. */
export function viewHasData(id: ViewId, shown: ShownReview): boolean {
  switch (id) {
    case "since": return (shown.commitsSinceReview?.length ?? 0) > 0;
    case "calls": return shown.overview?.flow != null;
    case "co-change": return shown.coChange.length > 0;
    case "connections": return shown.connections !== null;
    case "test-gaps": return shown.testGaps !== null;
    case "inputs": return shown.examples !== null;
  }
}

/** Whether the review's Views menu lists the view. */
export const viewListed = (view: ReviewView, shown: ShownReview) => !view.onlyWithData || viewHasData(view.id, shown);

/** Where this device keeps the views the user turned off, for every review. */
const VIEWS_OFF_PREFERENCE = "prompting-time.review-views-off";

function readViewsOff(): ViewId[] {
  try {
    const saved: unknown = JSON.parse(readPreference(VIEWS_OFF_PREFERENCE) ?? "[]");
    return Array.isArray(saved) ? REVIEW_VIEWS.filter((view) => saved.includes(view.id)).map((view) => view.id) : [];
  } catch {
    return [];
  }
}

export type ReviewViews = ReturnType<typeof createReviewViews>;

/** The views the user turned on or off, for every review, remembered on this device. */
export function createReviewViews() {
  const [off, setOff] = createSignal<readonly ViewId[]>(readViewsOff());
  return {
    isOn: (id: ViewId) => !off().includes(id),
    setOn(id: ViewId, on: boolean) {
      // From the pending value, not a read: Solid applies writes at the end of the batch.
      setOff((current) => {
        const next = on ? current.filter((item) => item !== id) : [...current.filter((item) => item !== id), id];
        savePreference(VIEWS_OFF_PREFERENCE, JSON.stringify(next));
        return next;
      });
    },
  };
}

const TOGGLE = "review-view:";

/** The palette's "Review views" group: turn each view on or off. */
export function viewCommands(views: ReviewViews): PaletteGroup[] {
  return [{
    heading: "Review views",
    items: REVIEW_VIEWS.map((view) => ({
      id: `${TOGGLE}${view.id}`,
      label: `${views.isOn(view.id) ? "Turn off" : "Turn on"} ${view.name}`,
      detail: view.description,
      keywords: ["views", "review", "show", "hide"],
    })),
  }];
}

/** Runs a "Review views" item; false for any other item, which the caller runs itself. */
export function runViewCommand(views: ReviewViews, id: string): boolean {
  const view = REVIEW_VIEWS.find((item) => id === `${TOGGLE}${item.id}`);
  if (!view) return false;
  views.setOn(view.id, !views.isOn(view.id));
  return true;
}
