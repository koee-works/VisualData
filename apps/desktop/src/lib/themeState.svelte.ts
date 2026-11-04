// The chosen theme, shared by everything that draws (charts, the editor).

import { apply, loadChoice, resolve, saveChoice, type ThemeChoice, type ThemeId } from "./theme";

export const themeState = $state<{ choice: ThemeChoice; id: ThemeId }>({ choice: loadChoice(), id: "paper" });

export function setTheme(choice: ThemeChoice) {
  themeState.choice = choice;
  saveChoice(choice);
  const t = resolve(choice);
  apply(t);
  themeState.id = t.id;
}

/** Apply the saved theme and follow the computer's light/dark setting. */
export function initTheme() {
  setTheme(themeState.choice);
  if (typeof matchMedia === "function") {
    matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
      if (themeState.choice === "system") setTheme("system");
    });
  }
}
