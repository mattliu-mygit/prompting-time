/// <reference types="vite/client" />
import { render } from "@solidjs/web";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import "@prompting-time/ui/styles.css";
import { App } from "./app/App";
import { createAppStore } from "./app/store";
import { installAppZoom } from "./app/zoom";
import * as api from "./bridge/api";
import "./styles/app.css";

const store = createAppStore(api);
const disposeZoom = installAppZoom((factor) => getCurrentWebview().setZoom(factor));
const disposeApp = render(() => <App store={store} />, document.getElementById("root")!);
window.addEventListener("beforeunload", () => {
  disposeApp();
  disposeZoom();
  store.dispose();
}, { once: true });
import.meta.hot?.dispose(() => {
  disposeApp();
  disposeZoom();
});
