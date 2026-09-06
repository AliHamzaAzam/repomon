/* @refresh reload */
import { render } from "solid-js/web";
import App from "./App";
import "./index.css";
import { applyTheme, readTheme } from "./theme";

applyTheme(readTheme());

render(() => <App />, document.getElementById("root") as HTMLElement);

