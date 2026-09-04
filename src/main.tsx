import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App";
import { createAppStore } from "./app/store";
import {
  getBootstrap,
  listConversations,
  listenToAppEvents,
  loadAgentTree,
  loadConversation,
} from "./bridge/api";
import "./styles/tokens.css";
import "./styles/app.css";

const store = createAppStore({
  getBootstrap,
  listConversations,
  loadConversation,
  loadAgentTree,
  listenToAppEvents,
});

window.addEventListener("beforeunload", () => store.dispose(), { once: true });

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App store={store} />
  </StrictMode>
);
