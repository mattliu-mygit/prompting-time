import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App";
import { getBootstrap } from "./bridge/api";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App bootstrap={getBootstrap} />
  </StrictMode>
);
