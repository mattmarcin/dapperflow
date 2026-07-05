// The terminal's color identity, matched to the DapperFlow console palette: a cool
// ink field, warm off-white text, a brass cursor, and a warm-leaning ANSI set so
// agent TUIs feel at home in the cockpit rather than dropped in from elsewhere.
import type { ITheme } from "@xterm/xterm";

export const terminalTheme: ITheme = {
  background: "#0F1217",
  foreground: "#E9E7E2",
  cursor: "#E6A23C",
  cursorAccent: "#0F1217",
  selectionBackground: "rgba(230, 162, 60, 0.28)",
  selectionForeground: "#0F1217",

  black: "#1B2027",
  red: "#E5686A",
  green: "#7BD0A8",
  yellow: "#E6A23C",
  blue: "#6C9CE6",
  magenta: "#C98BDB",
  cyan: "#5FBFC0",
  white: "#C7CAD1",

  brightBlack: "#3A424E",
  brightRed: "#F0898B",
  brightGreen: "#9BE0BE",
  brightYellow: "#F5BC5E",
  brightBlue: "#8FB4F0",
  brightMagenta: "#D8A6E6",
  brightCyan: "#82D4D4",
  brightWhite: "#F2F1EC",
};

export const TERMINAL_FONT =
  '"Cascadia Mono", "Cascadia Code", "JetBrains Mono", ui-monospace, "SF Mono", Consolas, "Liberation Mono", monospace';
