import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import "highlight.js/styles/github-dark.css";

declare global {
  interface Window {
    __ternionBoot?: (message: string) => void;
  }
}

window.__ternionBoot?.("boot: modules loaded");

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

window.__ternionBoot?.("boot: react mounted");