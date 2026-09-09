import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CopyButton, MessageContent } from "./MessageContent";

afterEach(() => { vi.useRealTimers(); });

describe("MessageContent", () => {
  it("keeps icon copy named and shows success and failure outside the button", async () => {
    const writeText = vi.fn().mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("unavailable"));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    render(<CopyButton content={"  exact source\n"} label="Copy preview" iconOnly />);
    const button = screen.getByRole("button", { name: "Copy preview" });
    expect(button).toHaveAttribute("title", "Copy preview");
    expect(button).toHaveClass("icon-button");
    expect(button.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    fireEvent.click(button);
    expect(await screen.findByRole("status")).toHaveTextContent("Copied");
    expect(button).not.toContainElement(screen.getByRole("status"));
    expect(writeText).toHaveBeenCalledWith("  exact source\n");
    fireEvent.click(button);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Could not copy. Try again."));
  });
  it("opens HTTP(S) links externally and keeps fragments local", () => {
    render(<MessageContent content="[http](http://example.com) [https](https://example.com) [section](#section)" />);
    for (const name of ["http", "https"]) {
      const link = screen.getByRole("link", { name });
      expect(link).toHaveAttribute("href", `${name}://example.com`);
      expect(link).toHaveAttribute("target", "_blank");
      expect(link).toHaveAttribute("rel", "noreferrer noopener");
    }
    const fragment = screen.getByRole("link", { name: "section" });
    expect(fragment).toHaveAttribute("href", "#section");
    expect(fragment).not.toHaveAttribute("target");
  });

  it("leaves non-HTTP(S) schemes inert", () => {
    render(<MessageContent content="[mail](mailto:person@example.com) [phone](tel:12345) [script](javascript:alert%281%29) [data](data:text/plain,hello) [file](file:///tmp/example.txt)" />);
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    for (const label of ["mail", "phone", "script", "data", "file"]) {
      expect(screen.getByText(label)).toBeVisible();
    }
  });

  it("does not report a previous copy as success for newly streamed content", async () => {
    let finish!: () => void;
    const writeText = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const view = render(<CopyButton content="old text" label="Copy message" />);
    fireEvent.click(screen.getByRole("button", { name: "Copy message" }));
    view.rerender(<CopyButton content="new text" label="Copy message" />);
    await act(async () => finish());
    expect(screen.queryByText("Copied")).not.toBeInTheDocument();
    expect(writeText).toHaveBeenCalledWith("old text");
  });
  it("renders Markdown formatting with inert HTML, unsafe links, and images", () => {
    const view = render(<MessageContent content={'# Heading\n\n> quote\n\n**strong** and *emphasis*\n\n<script>alert(1)</script>\n\n<iframe src="https://remote.invalid"></iframe>\n\n[unsafe](javascript:alert%281%29) [data](data:text/html,hello) [safe](https://example.com)\n\n![diagram](https://remote.invalid/image.png)'} />);
    expect(screen.getByRole("heading", { name: "Heading" })).toBeVisible();
    expect(view.container.querySelector("blockquote")).toHaveTextContent("quote");
    expect(view.container.querySelector("strong")).toHaveTextContent("strong");
    expect(view.container.querySelector("em")).toHaveTextContent("emphasis");
    expect(view.container.querySelector("script, iframe, img, object, embed")).toBeNull();
    expect(screen.getAllByRole("link")).toHaveLength(1);
    expect(screen.getByRole("link", { name: "safe" })).toHaveAttribute("href", "https://example.com");
    expect(screen.getByText("[Image: diagram]")).toBeVisible();
  });

  it("keeps unfinished and unlabeled code fences copyable with original code text", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const view = render(<MessageContent content={"```\n  exact <code>\n"} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("  exact <code>\n"));
    expect(view.container.querySelector("pre code")).toHaveTextContent("exact <code>");
    view.rerender(<MessageContent content={"```\n  exact <code>\n```\n\nnext paragraph"} />);
    expect(screen.getByText("next paragraph")).toBeVisible();
    expect(view.container.querySelectorAll("pre")).toHaveLength(1);
  });

  it("highlights settled bounded code and renders changed code immediately without stale colors", async () => {
    vi.useFakeTimers();
    const view = render(<MessageContent content={"```rust\nlet n = 1;\n```"} />);
    expect(view.container.querySelector(".hljs-keyword")).toBeNull();
    await act(async () => { vi.advanceTimersByTime(100); });
    expect(view.container.querySelector(".hljs-keyword")).toHaveTextContent("let");
    view.rerender(<MessageContent content={"```rust\nfn main() {}\n```"} />);
    expect(view.container.querySelector("pre")).toHaveTextContent("fn main() {}");
    expect(view.container.querySelector(".hljs-keyword")).toBeNull();
    await act(async () => { vi.advanceTimersByTime(100); });
    expect(view.container.querySelector(".hljs-keyword")).toHaveTextContent("fn");
  });

  it("leaves oversized and unknown-language code readable without highlighting", async () => {
    vi.useFakeTimers();
    const oversized = "let value = 1;\n".repeat(1200);
    const view = render(<MessageContent content={`\`\`\`rust\n${oversized}\`\`\``} />);
    await act(async () => { vi.advanceTimersByTime(100); });
    expect(view.container.querySelector("pre code")?.textContent).toBe(oversized);
    expect(view.container.querySelector(".hljs-keyword")).toBeNull();
    view.rerender(<MessageContent content={"```unknown-language\nraw code\n```"} />);
    await act(async () => { vi.advanceTimersByTime(100); });
    expect(view.container.querySelector("pre")).toHaveTextContent("raw code");
  });
});
