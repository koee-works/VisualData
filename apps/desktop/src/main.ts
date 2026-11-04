import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import { applyPrefs } from "./lib/prefs.svelte";

// Before the first paint, so the app never flashes the wrong colours.
applyPrefs();

const app = mount(App, { target: document.getElementById("app")! });
export default app;
