import { fireEvent, screen, waitFor } from "@testing-library/dom";
import { createSignal, flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { Markdown } from "./Markdown";
import { render } from "./test/render";

describe("Markdown", () => {
  it("opens HTTP(S) links in a new window and keeps fragments on the page", () => {
    render(() => <Markdown content="[http](http://example.com) [https](https://example.com) [section](#section)" />);
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

  it("leaves other link schemes as plain text", () => {
    render(() => <Markdown content="[mail](mailto:person@example.com) [phone](tel:12345) [script](javascript:alert%281%29) [data](data:text/plain,hello) [file](file:///tmp/example.txt)" />);
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    for (const label of ["mail", "phone", "script", "data", "file"]) expect(screen.getByText(label)).toBeVisible();
  });

  it("renders formatting and tables, drops HTML, and shows images as their alt text", () => {
    const container = render(() => (
      <Markdown
        content={'# Heading\n\n> quote\n\n**strong** and *emphasis*\n\n<script>alert(1)</script>\n\n<iframe src="https://remote.invalid"></iframe>\n\n[unsafe](javascript:alert%281%29) [safe](https://example.com)\n\n![diagram](https://remote.invalid/image.png)\n\n| Check | Result |\n| --- | --- |\n| Empty name | Rejected |'}
      />
    ));
    expect(screen.getByRole("heading", { name: "Heading" })).toBeVisible();
    expect(container.querySelector("blockquote")).toHaveTextContent("quote");
    expect(container.querySelector("strong")).toHaveTextContent("strong");
    expect(container.querySelector("em")).toHaveTextContent("emphasis");
    expect(container.querySelector("script, iframe, img, object, embed")).toBeNull();
    expect(screen.getAllByRole("link")).toHaveLength(1);
    expect(screen.getByText("[Image: diagram]")).toBeVisible();
    expect(screen.getByRole("cell", { name: "Rejected" }).closest(".pt-markdown-table")).not.toBeNull();
    // Tables hold only rows and cells, not the line breaks between them.
    const tableParts = [...container.querySelectorAll("table, thead, tbody, tr")];
    expect(tableParts.flatMap((part) => [...part.childNodes]).every((child) => child.nodeType === Node.ELEMENT_NODE)).toBe(true);
  });

  it("copies unfinished and unlabeled code fences as their exact text", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const [content, setContent] = createSignal("```\n  exact <code>\n");
    const container = render(() => <Markdown content={content()} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("  exact <code>\n"));
    expect(container.querySelector("pre code")).toHaveTextContent("exact <code>");
    setContent("```\n  exact <code>\n```\n\nnext paragraph");
    flush();
    expect(screen.getByText("next paragraph")).toBeVisible();
    expect(container.querySelectorAll("pre")).toHaveLength(1);
  });

  it("highlights code once it settles and shows changed code at once without old colors", async () => {
    const [content, setContent] = createSignal("```rust\nlet n = 1;\n```");
    const container = render(() => <Markdown content={content()} />);
    const keyword = () => [...container.querySelectorAll<HTMLElement>("pre code span")].find((span) => span.textContent === "let");
    expect(keyword()).toBeUndefined();
    await waitFor(() => expect(keyword()?.getAttribute("style")).toContain("--s-code-keyword-color"), { timeout: 5000 });
    setContent("```rust\nfn main() {}\n```\n\nstreamed paragraph");
    flush();
    expect(container.querySelector("pre")).toHaveTextContent("fn main() {}");
    expect(container.querySelector("pre code span")).toBeNull();
    await waitFor(() => expect([...container.querySelectorAll("pre code span")].some((span) => span.textContent === "fn")).toBe(true), { timeout: 5000 });
  });

  it("leaves oversized and unknown-language code unhighlighted", async () => {
    const oversized = "let value = 1;\n".repeat(1200);
    const [content, setContent] = createSignal(`\`\`\`rust\n${oversized}\`\`\``);
    const container = render(() => <Markdown content={content()} />);
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(container.querySelector("pre code")?.textContent).toBe(oversized);
    expect(container.querySelector("pre code span")).toBeNull();
    setContent("```unknown-language\nraw code\n```");
    flush();
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(container.querySelector("pre")).toHaveTextContent("raw code");
    expect(container.querySelector("pre code span")).toBeNull();
  });
});
