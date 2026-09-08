/// <reference types="vite/client" />
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { App } from "./app/App";
import { createAppStore } from "./app/store";
import { installAppZoom } from "./app/zoom";
import {
  getBootstrap,
  createConversation,
  archiveConversation,
  inspectWorkspace,
  inspectProject,
  pickProjectDirectory,
  interruptRun,
  listConversations,
  listenToAppEvents,
  loadApprovalDetail,
  loadApprovalQuestions,
  loadApprovals,
  loadAgentTree,
  loadConversation,
  loadEventDetail,
  loadTimeline,
  loadDiagnostics,
  listRunAudits,
  loadRunAudit,
  respondToApproval,
  steerRun,
  submitMessage,
} from "./bridge/api";
import "./styles/tokens.css";
import "./styles/app.css";

const store = createAppStore({
  getBootstrap,
  createConversation,
  archiveConversation,
  listConversations,
  loadConversation,
  loadAgentTree,
  listenToAppEvents,
  loadTimeline,
  loadDiagnostics,
  listRunAudits,
  loadRunAudit,
  loadEventDetail,
  loadApprovals,
  loadApprovalDetail,
  loadApprovalQuestions,
  submitMessage,
  steerRun,
  respondToApproval,
  interruptRun,
  inspectWorkspace,
  inspectProject,
  pickProjectDirectory,
});

const disposeZoom = installAppZoom((factor) => getCurrentWebview().setZoom(factor));
window.addEventListener("beforeunload", () => {
  disposeZoom();
  store.dispose();
}, { once: true });
import.meta.hot?.dispose(disposeZoom);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App store={store} />
  </StrictMode>
);
