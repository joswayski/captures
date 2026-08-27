import React from "react";
import ReactDOM from "react-dom/client";

import { App } from "./App";
import "./styles.css";
import { applyAppearance, readStoredAppearance } from "../../../../shared/appearance";
import {
  applyColorTheme,
  readStoredColorTheme,
  readStoredCustomTheme,
} from "../../../../shared/themes";

applyAppearance(readStoredAppearance());
applyColorTheme(readStoredColorTheme(), readStoredCustomTheme());

async function start() {
  // Dev-only design harness: renders any window with representative data so the
  // UI can be reviewed in a browser without the Tauri backend.
  if (import.meta.env.DEV && new URLSearchParams(window.location.search).has("mock")) {
    const { installPreviewBackend } = await import("./dev/previewBackend");
    installPreviewBackend();
  }

  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

void start();
