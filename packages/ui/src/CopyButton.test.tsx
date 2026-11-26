import { fireEvent, screen, waitFor } from "@testing-library/dom";
import { createSignal, flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { CopyButton } from "./CopyButton";
import { axeViolations, render } from "./test/render";

describe("CopyButton", () => {
  it("names an icon-only button and shows success and failure outside it", async () => {
    const writeText = vi.fn().mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("unavailable"));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    render(() => <CopyButton content={"  exact source\n"} label="Copy preview" iconOnly />);
    const button = screen.getByRole("button", { name: "Copy preview" });
    expect(button).toHaveAttribute("title", "Copy preview");
    expect(button.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    fireEvent.click(button);
    expect(await screen.findByRole("status")).toHaveTextContent("Copied");
    expect(button).not.toContainElement(screen.getByRole("status"));
    expect(writeText).toHaveBeenCalledWith("  exact source\n");
    fireEvent.click(button);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Could not copy. Try again."));
    expect(await axeViolations()).toEqual([]);
  });

  it("does not report a copy of earlier text as success for newer text", async () => {
    let finish!: () => void;
    const writeText = vi.fn(() => new Promise<void>((resolve) => (finish = resolve)));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const [content, setContent] = createSignal("old text");
    render(() => <CopyButton content={content()} label="Copy message" />);
    fireEvent.click(screen.getByRole("button", { name: "Copy message" }));
    setContent("new text");
    flush();
    finish();
    await Promise.resolve();
    flush();
    expect(screen.queryByText("Copied")).not.toBeInTheDocument();
    expect(writeText).toHaveBeenCalledWith("old text");
  });
});
