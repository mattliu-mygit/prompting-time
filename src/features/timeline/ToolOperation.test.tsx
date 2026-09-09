import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { EventDetailSnapshot, TimelineItem } from "../../bridge/types";
import { TimelineEntry } from "./TimelineEntry";

const item: TimelineItem = { id: "operation", sequence: "1", conversationId: "conversation", runId: "run", agentId: "root", provider: "codex", kind: "tool", presentation: "normal", role: null, content: "Run cargo test", contentBytes: "14", truncated: false,
  operation: { status: "running", hasDetails: true, detailRevision: "1", durationMs: "1250", exitCode: null, truncated: false, conflicted: false } };
const detail: EventDetailSnapshot = { id: item.id, content: item.content, contentBytes: "14", truncated: false,
  operation: { title: item.content, status: "running", input: "cargo test", output: "1 test passed\n", error: null, context: "/invented", durationMs: "1250", exitCode: null, truncated: false, conflicted: false, detailRevision: "1" } };

describe("Tool operation", () => {
  it("shows an evidence-backed summary and fetches distinct inert details only on disclosure", async () => {
    const loadEventDetail = vi.fn().mockResolvedValue(detail);
    render(<TimelineEntry item={item} actions={{ loadEventDetail }} agentPath="Root" />);
    expect(screen.getByText("Running")).toBeVisible();
    expect(screen.getByText("1.25 s")).toBeVisible();
    expect(screen.queryByText("cargo test")).not.toBeInTheDocument();
    expect(loadEventDetail).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Show details" }));
    expect(await screen.findByText("1 test passed")).toBeVisible();
    expect(screen.getByText("cargo test").tagName).toBe("PRE");
    expect(loadEventDetail).toHaveBeenCalledExactlyOnceWith({ eventId: item.id });
  });

  it("never promises uncaptured detail for modern or legacy operations", () => {
    const loadEventDetail = vi.fn();
    const view = render(<TimelineEntry item={{ ...item, operation: { ...item.operation!, hasDetails: false } }} actions={{ loadEventDetail }} />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getByText("Details weren't captured")).toBeVisible();
    view.rerender(<TimelineEntry item={{ ...item, operation: null, content: "reasoning: observed" }} actions={{ loadEventDetail }} />);
    expect(screen.getByText("reasoning: observed")).toBeVisible();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getByText("Details weren't captured")).toBeVisible();
  });

  it.each([
    { output: null, truncated: true, explanation: "Output wasn't captured" },
    { output: "", truncated: true, explanation: "No output retained; captured detail was truncated" },
    { output: "", truncated: false, explanation: "Output was empty" },
  ])("distinguishes output $output truncated=$truncated and copies only captured fields", async ({ output, truncated, explanation }) => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    render(<TimelineEntry item={item} actions={{ loadEventDetail: vi.fn().mockResolvedValue({ ...detail, operation: { ...detail.operation!, output, truncated } }) }} />);
    fireEvent.click(screen.getByRole("button", { name: "Show details" }));
    expect(await screen.findByText(explanation)).toBeVisible();
    expect(screen.queryByRole("button", { name: /Copy output/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: truncated ? "Copy input preview" : "Copy input" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledExactlyOnceWith("cargo test"));
    if (truncated) expect(screen.getByText("Captured detail is truncated.")).toBeVisible();
    else expect(screen.queryByText("Captured detail is truncated.")).not.toBeInTheDocument();
  });

  it("keeps detail open across revisions and ignores a stale response after collapse", async () => {
    let finish!: (value: EventDetailSnapshot) => void;
    const loadEventDetail = vi.fn().mockResolvedValueOnce(detail).mockImplementationOnce(() => new Promise<EventDetailSnapshot>(resolve => { finish = resolve; }));
    const view = render(<TimelineEntry item={item} actions={{ loadEventDetail }} />);
    fireEvent.click(screen.getByRole("button", { name: "Show details" }));
    await screen.findByText("1 test passed");
    view.rerender(<TimelineEntry item={{ ...item, operation: { ...item.operation!, detailRevision: "2", status: "succeeded" } }} actions={{ loadEventDetail }} />);
    expect(screen.getByRole("button", { name: "Hide details" })).toHaveAttribute("aria-expanded", "true");
    await waitFor(() => expect(loadEventDetail).toHaveBeenCalledTimes(2));
    fireEvent.click(screen.getByRole("button", { name: "Hide details" }));
    await act(async () => finish({ ...detail, operation: { ...detail.operation!, output: "late output", detailRevision: "2" } }));
    expect(screen.queryByText("late output")).not.toBeInTheDocument();
  });

  it("offers an explicit retry after a detail read failure", async () => {
    const loadEventDetail = vi.fn().mockRejectedValueOnce(new Error("Read failed")).mockResolvedValueOnce(detail);
    render(<TimelineEntry item={item} actions={{ loadEventDetail }} />);
    fireEvent.click(screen.getByRole("button", { name: "Show details" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Read failed");
    fireEvent.click(screen.getByRole("button", { name: "Retry detail" }));
    expect(await screen.findByText("1 test passed")).toBeVisible();
  });

  it("copies exact inert output and rejects mismatched detail revisions", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const output = "<script>invented()</script>\n  result\n";
    const loadEventDetail = vi.fn().mockResolvedValueOnce({ ...detail, operation: { ...detail.operation!, output, detailRevision: "0" } }).mockResolvedValueOnce({ ...detail, operation: { ...detail.operation!, output } });
    const view = render(<TimelineEntry item={item} actions={{ loadEventDetail }} />);
    fireEvent.click(screen.getByRole("button", { name: "Show details" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Activity changed while loading");
    fireEvent.click(screen.getByRole("button", { name: "Retry detail" }));
    fireEvent.click(await screen.findByRole("button", { name: "Copy output" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledExactlyOnceWith(output));
    expect(view.container.querySelector("script")).toBeNull();
  });

  it("discards a response after unmount without contaminating a different operation", async () => {
    let finish!: (value: EventDetailSnapshot) => void;
    const loadEventDetail = vi.fn().mockImplementationOnce(() => new Promise<EventDetailSnapshot>(resolve => { finish = resolve; }));
    const view = render(<TimelineEntry item={item} actions={{ loadEventDetail }} />);
    fireEvent.click(screen.getByRole("button", { name: "Show details" }));
    view.unmount();
    render(<TimelineEntry item={{ ...item, id: "different" }} actions={{ loadEventDetail }} />);
    await act(async () => finish(detail));
    expect(screen.queryByText("1 test passed")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show details" })).toHaveAttribute("aria-expanded", "false");
  });
});
