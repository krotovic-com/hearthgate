import { mount } from "svelte";
import "./styles/base.css";
import App from "./App.svelte";
import { applyTheme, storedTheme } from "./lib/theme";

applyTheme(storedTheme());

const target = document.getElementById("app");
if (!target) throw new Error("missing #app");
export default mount(App, { target });
