import React from "react";
import ReactDOM from "react-dom/client";

import { App } from "./App";
import { installPreviewIfNeeded } from "./preview/installPreview";
import "./styles.css";
import {
  applyColorTheme,
  readStoredColorTheme,
  readStoredCustomTheme,
} from "../../../../shared/themes";

applyColorTheme(readStoredColorTheme(), readStoredCustomTheme());
installPreviewIfNeeded();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
