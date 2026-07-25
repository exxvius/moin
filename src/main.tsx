import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { initPerfMode } from "./lib/perfMode";
import "./styles/fonts.css";
import "./styles/global.css";
import "./styles/app.css";
import "./styles/moin.css";
// Last, so its overrides win on equal specificity.
import "./styles/performance.css";

// Before the first render, so performance mode never flashes a frame of glass.
initPerfMode();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
