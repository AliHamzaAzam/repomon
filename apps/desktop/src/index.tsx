/* @refresh reload */
import { render } from "solid-js/web";
import App from "./App";
import "./index.css";
import { applyTheme, readTheme } from "./theme";

// Immediately apply the saved theme to document.documentElement on boot
applyTheme(readTheme());

render(() => <App />, document.getElementById("root") as HTMLElement);

