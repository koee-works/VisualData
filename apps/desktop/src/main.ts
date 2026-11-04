import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import { initTheme } from "./lib/themeState.svelte";

// Before the first paint, so the app never flashes the wrong colours.
initTheme();

const app = mount(App, { target: document.getElementById("app")! });
export default app;
