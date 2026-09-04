import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App";
import { createAppStore } from "./app/store";
import {
  getBootstrap,
  createConversation,
  archiveConversation,
  inspectWorkspace,
  inspectProject,
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
});

window.addEventListener("beforeunload", () => store.dispose(), { once: true });

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App store={store} />
  </StrictMode>
);
