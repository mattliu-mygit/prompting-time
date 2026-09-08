import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { TimelineItem } from "../../bridge/types";
import { Diagnostics } from "./Diagnostics";

function event(id: string, sequence = "1"): TimelineItem {
  return { id, sequence, conversationId: "conversation", runId: "run", agentId: "root", provider: "codex",
    kind: "diagnostic", presentation: "telemetry", role: null, content: `preview ${id}`, contentBytes: "1000", truncated: true };
}

describe("Diagnostics", () => {
  it("keeps collapsed reads at zero and retries the failed older cursor without losing newer entries", async () => {
    const actions = { loadEventDetail: vi.fn(), loadDiagnostics: vi.fn()
      .mockResolvedValueOnce({ items: [event("new", "2")], nextCursor: "older" })
      .mockRejectedValueOnce(new Error("read interrupted"))
      .mockResolvedValueOnce({ items: [event("old")], nextCursor: null }) };
    const view = render(<Diagnostics conversationId="conversation" actions={actions} />);
    view.rerender(<Diagnostics conversationId="conversation" actions={{ ...actions }} />);
    expect(actions.loadDiagnostics).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Expand diagnostics" }));
    expect(await screen.findByText("preview new")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Load older diagnostics" }));
    expect(await screen.findByText("read interrupted")).toBeVisible();
    expect(screen.getByText("preview new")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Retry diagnostics" }));
    expect(await screen.findByText("preview old")).toBeVisible();
    expect(actions.loadDiagnostics).toHaveBeenLastCalledWith({ conversationId: "conversation", cursor: "older", limit: 30 });
    expect(screen.getAllByRole("article")[0]).toHaveTextContent("preview old");
    expect(actions.loadEventDetail).not.toHaveBeenCalled();
  });

  it("discards pending event detail when diagnostics closes and reopens", async () => {
    let finish!: (result: { id: string; content: string; contentBytes: string; truncated: boolean }) => void;
    const actions = { loadDiagnostics: vi.fn().mockResolvedValue({ items: [event("one")], nextCursor: null }),
      loadEventDetail: vi.fn(() => new Promise<{ id: string; content: string; contentBytes: string; truncated: boolean }>((resolve) => { finish = resolve; })) };
    render(<Diagnostics conversationId="conversation" actions={actions} />);
    fireEvent.click(screen.getByRole("button", { name: "Expand diagnostics" }));
    fireEvent.click(await screen.findByRole("button", { name: "Show full provider activity" }));
    fireEvent.click(screen.getByRole("button", { name: "Collapse diagnostics" }));
    fireEvent.click(screen.getByRole("button", { name: "Expand diagnostics" }));
    await screen.findByText("preview one");
    await act(async () => finish({ id: "one", content: "stale exact detail", contentBytes: "18", truncated: false }));
    expect(screen.queryByText("stale exact detail")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show full provider activity" })).toHaveAttribute("aria-expanded", "false");
    await waitFor(() => expect(actions.loadDiagnostics).toHaveBeenCalledTimes(2));
  });
});
