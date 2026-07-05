import ReactDOM from "react-dom/client";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";
import { App } from "./App";

// No StrictMode: the terminal panes own imperative xterm instances, and the dev
// double-invoke would churn attach/detach against real daemon sessions.
const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);
root.render(<App />);
