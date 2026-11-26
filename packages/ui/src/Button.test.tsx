import { fireEvent, screen } from "@testing-library/dom";
import { describe, expect, it, vi } from "vitest";
import { Button, IconButton } from "./Button";
import { axeViolations, render } from "./test/render";

describe("Button and IconButton", () => {
  it("are plain buttons that never submit a form by accident", async () => {
    const onClick = vi.fn();
    render(() => (
      <form>
        <Button variant="primary" onClick={onClick}>
          Submit review
        </Button>
        <Button>Retry now</Button>
        <Button disabled>Save</Button>
        <IconButton label="Search">
          <svg aria-hidden="true" />
        </IconButton>
      </form>
    ));
    const primary = screen.getByRole("button", { name: "Submit review" });
    expect(primary).toHaveAttribute("type", "button");
    expect(primary).toHaveAttribute("data-variant", "primary");
    expect(screen.getByRole("button", { name: "Retry now" })).toHaveAttribute("data-variant", "plain");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Search" })).toHaveAttribute("title", "Search");
    fireEvent.click(primary);
    expect(onClick).toHaveBeenCalledOnce();
    expect(await axeViolations()).toEqual([]);
  });
});
