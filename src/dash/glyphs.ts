/** The glyph sets: Unicode, and the ASCII fallback used for TERM=dumb and non-UTF-8 locales. */

export interface Edge {
  readonly tl: string;
  readonly tr: string;
  readonly bl: string;
  readonly br: string;
  readonly h: string;
  readonly v: string;
}

export interface Glyphs {
  readonly ascii: boolean;
  readonly edge: Edge;
  readonly edgeFocused: Edge;
  readonly tabOpen: string;
  readonly tabClose: string;
  /** Hotkey number prefix for panel 1 to 5 (index 0 unused). */
  readonly hotkey: readonly string[];
  readonly spinner: string;
  readonly working: string;
  readonly idle: string;
  readonly attention: string;
  readonly ended: string;
  readonly linkUp: string;
  readonly linkDown: string;
  readonly full: string;
  readonly empty: string;
  readonly stageGood: string;
  readonly stageProgress: string;
  readonly stageBad: string;
  readonly selected: string;
  readonly changed: string;
  readonly thumb: string;
  readonly arrow: string;
  readonly rule: string;
  readonly notified: string;
  readonly notNotified: string;
  readonly up: string;
  readonly down: string;
}

const UNICODE: Glyphs = {
  ascii: false,
  edge: { tl: "╭", tr: "╮", bl: "╰", br: "╯", h: "─", v: "│" },
  edgeFocused: { tl: "┏", tr: "┓", bl: "┗", br: "┛", h: "━", v: "┃" },
  tabOpen: "┤",
  tabClose: "├",
  hotkey: ["", "¹", "²", "³", "⁴", "⁵"],
  spinner: "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏",
  working: "*",
  idle: "●",
  attention: "▲",
  ended: "○",
  linkUp: "●",
  linkDown: "○",
  full: "█",
  empty: "░",
  stageGood: "█",
  stageProgress: "▓",
  stageBad: "▒",
  selected: "▌",
  changed: "+",
  thumb: "█",
  arrow: "──►",
  rule: "─",
  notified: "●",
  notNotified: "·",
  up: "↑",
  down: "↓",
};

const ASCII: Glyphs = {
  ascii: true,
  edge: { tl: "+", tr: "+", bl: "+", br: "+", h: "-", v: "|" },
  edgeFocused: { tl: "+", tr: "+", bl: "+", br: "+", h: "=", v: "|" },
  tabOpen: "[",
  tabClose: "]",
  hotkey: ["", "1.", "2.", "3.", "4.", "5."],
  spinner: "|/-\\",
  working: "*",
  idle: "o",
  attention: "!",
  ended: "-",
  linkUp: "*",
  linkDown: "o",
  full: "#",
  empty: ".",
  stageGood: "#",
  stageProgress: "+",
  stageBad: "x",
  selected: ">",
  changed: "+",
  thumb: "#",
  arrow: "->",
  rule: "-",
  notified: "*",
  notNotified: ".",
  up: "up",
  down: "down",
};

export function glyphsFor(ascii: boolean): Glyphs {
  return ascii ? ASCII : UNICODE;
}
