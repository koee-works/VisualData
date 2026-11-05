// Every keyboard shortcut in the app, for Settings > Keyboard. The keys
// themselves are set where they work (TopBar.svelte, editor/setup.ts); this
// is the list people read, so a change there needs a change here.

export interface Shortcut {
  /** The keys, written for this computer ("Ctrl+K" or "⌘K"). */
  keys: string;
  what: string;
}

export interface ShortcutGroup {
  name: string;
  items: Shortcut[];
}

type Combo = { key: string; mod?: boolean; shift?: boolean; alt?: boolean };

/** A key combination as this computer writes it. */
export function combo(c: Combo, mac: boolean): string {
  if (mac) return `${c.alt ? "⌥" : ""}${c.shift ? "⇧" : ""}${c.mod ? "⌘" : ""}${c.key}`;
  return [c.mod ? "Ctrl" : "", c.alt ? "Alt" : "", c.shift ? "Shift" : "", c.key].filter(Boolean).join("+");
}

export function shortcutGroups(mac: boolean): ShortcutGroup[] {
  const k = (key: string, o: Omit<Combo, "key"> = {}) => combo({ key, ...o }, mac);
  const either = (a: string, b: string) => `${a} or ${b}`;
  return [
    {
      name: "Anywhere",
      items: [
        { keys: either(k("K", { mod: true }), k("P", { mod: true, shift: true })), what: "Open the command palette: every action by name" },
        { keys: k(",", { mod: true }), what: "Open Settings" },
        { keys: k("Z", { mod: true }), what: "Undo the last change (not while typing in a text field)" },
        { keys: mac ? k("Z", { mod: true, shift: true }) : either(k("Y", { mod: true }), k("Z", { mod: true, shift: true })), what: "Redo" },
      ],
    },
    {
      name: "Panels",
      items: [
        { keys: k("B", { mod: true }), what: "Show or hide the pipeline" },
        { keys: k("J", { mod: true }), what: "Show or hide the Activity panel" },
        { keys: k("`", { mod: true }), what: "Show or hide the console (beside a Python or R step)" },
      ],
    },
    {
      name: "Code editor",
      items: [
        { keys: k("Enter", { shift: true }), what: "Save the step and run it" },
        { keys: k("Enter", { mod: true }), what: "Python and R: run the line or selection in the console. SQL: save and run the step" },
        { keys: k("Enter", { mod: true, shift: true }), what: "Python and R: run all the code in the console" },
        // Control on every computer, as in the editor (⌘Space is the Mac's search).
        { keys: mac ? "⌃Space" : "Ctrl+Space", what: "Suggest names (tables, columns, functions)" },
        { keys: k("F", { mod: true }), what: "Find in the code" },
        { keys: "Tab", what: "Indent the selected lines" },
        { keys: k("-", { alt: true }), what: "R: type <-" },
        { keys: k("M", { mod: true, shift: true }), what: "R: type |>" },
      ],
    },
    {
      name: "Console and palette",
      items: [
        { keys: "Enter", what: "Console: run the line. Palette: do the highlighted action" },
        { keys: either("↑", "↓"), what: "Console: earlier entries. Palette: move through the list" },
        { keys: "Esc", what: "Close the palette or a dialog" },
      ],
    },
  ];
}
