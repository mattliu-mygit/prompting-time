import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";

const installedProviders = {
  providers: [
    { id: "codex" as const, installed: true, version: "0.144.1", diagnostic: null },
    { id: "claude" as const, installed: true, version: "2.1.205", diagnostic: null }
  ]
};

describe("App", () => {
  it("shows both provider diagnostics", async () => {
    const bootstrap = vi.fn().mockResolvedValue({
      providers: [
        { id: "codex", installed: true, version: "0.144.1", diagnostic: null },
        { id: "claude", installed: true, version: "2.1.205", diagnostic: null }
      ]
    });

    render(<App bootstrap={bootstrap} />);

    expect(await screen.findByText("Codex 0.144.1")).toBeVisible();
    expect(screen.getByText("Claude 2.1.205")).toBeVisible();
  });

  it("shows loading while bootstrap is pending", () => {
    render(<App bootstrap={() => new Promise(() => {})} />);

    expect(screen.getByText("Checking installed providers…")).toBeVisible();
  });

  it("shows an unavailable provider diagnostic", async () => {
    render(
      <App
        bootstrap={async () => ({
          providers: [
            {
              id: "codex",
              installed: false,
              version: null,
              diagnostic: "codex was not found"
            }
          ]
        })}
      />
    );

    expect(await screen.findByText("Codex unavailable: codex was not found")).toBeVisible();
  });

  it("shows a rejected bootstrap error", async () => {
    render(<App bootstrap={async () => Promise.reject(new Error("Bridge disconnected"))} />);

    expect(
      await screen.findByText("Could not inspect installed providers: Bridge disconnected")
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Retry" })).toBeVisible();
  });

  it("retries bootstrap after a rejected request", async () => {
    let calls = 0;
    const bootstrap = async () => {
      calls += 1;
      if (calls === 1) {
        throw new Error("Bridge disconnected");
      }
      return installedProviders;
    };
    render(<App bootstrap={bootstrap} />);

    fireEvent.click(await screen.findByRole("button", { name: "Retry" }));

    expect(await screen.findByText("Codex 0.144.1")).toBeVisible();
  });
});
