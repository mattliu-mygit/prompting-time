// The ?agents=v2 fixture's plugin page, which speaks page protocol 2
// (src/features/plugins/protocol.ts): it says ready, shows the route Prompting Time opens, reads
// the status with `call`, reads again when files change, and offers Rerun, Delete (which asks to
// confirm first), and a call to an action the manifest does not declare, which Prompting Time
// refuses. It is plain script, so the fixture needs no build.
"use strict";

const VERSION = 2;
const pending = new Map();
let nextId = 1;
let route = null;
let lastStatus = null;

const element = (id) => document.getElementById(id);

function log(text) {
  const lines = element("log").textContent.split("\n").filter(Boolean);
  element("log").textContent = [...lines, text].slice(-12).join("\n");
}

function call(action, args = []) {
  const id = nextId++;
  parent.postMessage({ version: VERSION, type: "call", id, action, args }, "*");
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

/** The run the open route names, such as run-74c91b1d for /runs/run-74c91b1d, or null. */
function openRun() {
  const match = route && /^\/runs\/([^/]+)$/.exec(route);
  return match ? match[1] : null;
}

function show() {
  element("route").textContent = `Route: ${route ?? "none yet"}`;
  const id = openRun();
  const item = lastStatus && id ? lastStatus.items.find((entry) => entry.id === id) : null;
  const blocker = lastStatus && route ? lastStatus.blockers.find((entry) => entry.route === route) : null;
  element("title").textContent = item?.title ?? blocker?.title ?? (route === "/providers" ? "Providers" : "Fixture runs");
  element("detail").textContent = item ? [item.group, item.detail].filter(Boolean).join(" · ")
    : id ? "This run is no longer listed." : blocker ? "Blocks every run." : "Waiting for Prompting Time to open a run.";
  element("summary").textContent = lastStatus ? `${lastStatus.items.length} runs listed, ${lastStatus.blockers.length} blocking problems.` : "";
  element("rerun").disabled = !item;
  element("delete").disabled = !item;
}

async function readStatus() {
  try {
    lastStatus = await call("status");
    show();
  } catch (reason) {
    log(`status failed: ${reason.message}`);
  }
}

async function runOnOpenRun(action) {
  const id = openRun();
  if (!id) return;
  try {
    log(`${action}: ${String(await call(action, ["--item", id])).trim()}`);
  } catch (reason) {
    log(`${action}: ${reason.message}`);
  }
}

window.addEventListener("message", (event) => {
  const message = event.data;
  if (event.source !== parent || !message || message.version !== VERSION) return;
  switch (message.type) {
    case "result":
    case "error": {
      const waiting = pending.get(message.reply_to);
      if (!waiting) {
        if (message.type === "error") log(`error: ${message.message}`);
        return;
      }
      pending.delete(message.reply_to);
      if (message.type === "result") waiting.resolve(message.json);
      else waiting.reject(new Error(message.message));
      return;
    }
    case "open":
      route = message.route;
      log(`open ${route}`);
      show();
      void readStatus();
      return;
    case "changed":
      element("changed").textContent = `Changed: ${message.files === null ? "more files than one event lists" : message.files.join(", ") || "the plugin itself"}`;
      void readStatus();
      return;
  }
});

element("refresh").addEventListener("click", () => void readStatus());
element("rerun").addEventListener("click", () => void runOnOpenRun("rerun"));
element("delete").addEventListener("click", () => void runOnOpenRun("delete"));
element("undeclared").addEventListener("click", () => {
  call("rm-everything").then(() => log("rm-everything ran, which it never should"), (reason) => log(`rm-everything: ${reason.message}`));
});

show();
parent.postMessage({ version: VERSION, type: "ready" }, "*");
