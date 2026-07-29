import React from "react";
import ReactDOM from "react-dom/client";

import { App } from "./App";
import "./styles.css";
import { applyColorTheme, readStoredColorTheme } from "../../../../shared/themes";

applyColorTheme(readStoredColorTheme());

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
