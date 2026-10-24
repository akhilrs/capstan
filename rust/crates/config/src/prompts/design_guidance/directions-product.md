# Product directions

Eight directions for tools and information products. Each entry fills every field; a proposal repeats the fields for each direction it presents. Directions are starting points: each design must still pass the rules unless the brief or design system sets another value, and then the deviation is listed. Blend at most two.

## Ledger
F1: mood (three words, one-line idea): calm, exact, trustworthy; a table-first interface where the numbers are the hero
F2: surface polarity: light by default with a full dark alternative; one polarity per session, never mixed panels
F3: accent discipline: one accent for the primary action, the selected row and links; never for status, headings or secondary button fills
F4: neutrals: cool-tinted greys in 7 steps: page, panel, stripe, hairline, muted text, text, strong text
F5: type roles and sizes per surface: one neutral sans for all text, mono only for ids and code; labels 13 px, cells 14 px, page title 24 px; tabular figures; no marketing surface unless the brief adds one
F6: shape language: radius 2 to 4 px on controls, 0 on tables, pills only for tags
F7: depth strategy: hairlines and tonal row stripes; shadows only on menus
F8: spacing unit and density per surface: 4 px unit and high density with 28 to 36 px rows; a marketing page, if any, uses an 8 px unit and medium density
F9: imagery role: the data is the imagery (sparklines, status marks); no photography
F10: signature moves: sticky header and first column; right-aligned numerals with units in the header; filter chips above the table
F11: forbidden moves (3-6, testable): no stripes plus borders together; no row taller than 44 px; no accent on more than one element per row; no decorative charts; no centred table cells
F12: responsive plan: narrow screens keep the key column and move the rest into a row detail; wide tables scroll inside their frame; targets 24 px minimum, 44 px on touch
F13: state coverage: hover row highlight, focus ring on cells and rows, selected, disabled, loading skeleton rows, empty with a next step, error row with retry
F14: fallbacks: system sans and system mono; if tabular figures are unavailable, align by the right edge
F15: motion: feedback only: row highlight 100 ms, sort change 150 ms, one easing family; reduced motion drops slides and keeps fades

## Control Room
F1: mood (three words, one-line idea): alert, focused, undecorated; the system status is the loudest thing on screen
F2: surface polarity: dark by default with a light variant for bright rooms; status hues re-tuned for each
F3: accent discipline: a muted accent for focus and selection only; status colours are separate and never reused as the accent
F4: neutrals: blue-grey in 8 steps from near-black to light grey; panels separated by tone, not borders
F5: type roles and sizes per surface: compact sans for labels, mono for values and logs; panel titles 12 to 13 px caps, letter-spaced; values 20 to 32 px; log text 13 px
F6: shape language: square corners, 2 px at most; status marks are circles or squares by meaning
F7: depth strategy: tonal steps: each panel one step lighter than the page; no shadows
F8: spacing unit and density per surface: 4 px unit and high density with 8 to 12 px panel padding, the same on every surface
F9: imagery role: none; live charts and status glyphs carry the look
F10: signature moves: a status strip across the top; resizable, collapsible panels; a keyboard command palette
F11: forbidden moves (3-6, testable): no decorative gradients; no status colour used as decoration; no animation on idle panels; no more than 4 status hues; no truncated alert titles
F12: responsive plan: panels stack by priority with alerts first; targets 24 px minimum, 44 px on touch; wide logs scroll in a frame
F13: state coverage: hover, visible focus ring on dark, active, disabled, loading (stale marker plus age), empty ("no events since ..."), error (source unreachable with retry)
F14: fallbacks: system sans and mono; a condensed face is replaced by one of the same width class
F15: motion: only data changes flash (150 ms highlight, then settle); no looping motion; reduced motion shows a static marker

## Workbench
F1: mood (three words, one-line idea): capable, quiet, focused; the chrome recedes behind the work
F2: surface polarity: light or dark by user choice, with a neutral canvas in both
F3: accent discipline: accent for selection, focus and the primary action only; never on panel headers or resting icons
F4: neutrals: 5 to 6 neutral steps lightly tinted toward the accent hue; the canvas is its own step apart from panels
F5: type roles and sizes per surface: UI sans only: labels 13 to 14 px, descriptions 15 px or more, panel titles 13 px medium; optional mono for values; no display face
F6: shape language: radius 4 to 6 px on controls, 0 on panels docked to an edge
F7: depth strategy: tonal steps for panels; one soft shadow only for floating palettes and menus
F8: spacing unit and density per surface: 4 px unit and medium-high density with a 40 px toolbar; a marketing page for the tool uses an 8 px unit and medium density
F9: imagery role: the work itself carries it: real documents on the marketing page, one outline icon style in the tool
F10: signature moves: collapsible side panels; a contextual toolbar that follows the selection; command search
F11: forbidden moves (3-6, testable): no icon without a label or tooltip; no more than two toolbars; no accent-filled panels; no modal for an edit that can happen inline; no animation while panning the canvas
F12: responsive plan: side panels become drawers below about 900 px and the canvas keeps priority; tool palettes dock at the bottom on touch; targets 44 px on touch
F13: state coverage: hover, focus, active, selected, disabled, in-place loading, empty canvas with a starter action, error with undo
F14: fallbacks: system sans; name the substitute icon set; note features that need pointer precision
F15: motion: panels open and close in 150 to 200 ms with one easing; selection feedback is instant; reduced motion switches panels without a slide

## Instrument
F1: mood (three words, one-line idea): engineered, exact, live; readouts you can trust at a glance
F2: surface polarity: dark for low-light settings, light for labs; state which, never mixed within a view
F3: accent discipline: one signal colour for live values and the primary action; faults use state colours only
F4: neutrals: industrial neutrals in 6 steps, warm or cool but consistent
F5: type roles and sizes per surface: grotesque sans for labels, mono for readouts and units; readouts 28 to 48 px, labels 12 to 13 px small caps, units at 60% of the value size
F6: shape language: square or 2 px radius; gauges and dials are circles because of their function
F7: depth strategy: hairlines and tonal steps; a bezel-like inset is the one allowed decorative device
F8: spacing unit and density per surface: 4 px unit on fixed grid cells, medium-high density; a marketing page uses an 8 px unit
F9: imagery role: diagrams and real device photographs; no stock imagery
F10: signature moves: units beside every value; tick-mark scales; a live-value pulse that shows data age
F11: forbidden moves (3-6, testable): no value without a unit; no rounding that hides precision; no red for anything but a fault; no skeuomorphic chrome on controls; no needle that lags the data
F12: responsive plan: the readout grid reflows from 4 to 2 to 1 columns by importance; targets 44 px; tables scroll in a frame
F13: state coverage: hover, focus, active, disabled, stale (data age), loading, empty (no sensor), error (fault with code)
F14: fallbacks: a system grotesque plus system mono; keep tabular figures
F15: motion: values tick without easing over 100 ms; no looping; reduced motion updates without a highlight

## Briefing
F1: mood (three words, one-line idea): concise, confident, decisive; the one number that matters comes first
F2: surface polarity: light by default with an optional dark for presentations
F3: accent discipline: the accent points at the one thing that changed; never decoration, never more than one item per section
F4: neutrals: warm or cool tinted neutrals in 5 steps with wide white space
F5: type roles and sizes per surface: headline 32 to 48 px, key figure 40 to 64 px in one numeric style, supporting text 15 to 16 px, captions 13 px; one family across surfaces
F6: shape language: radius 4 to 8 px; cards only for the main figures
F7: depth strategy: hairlines and white space; no shadows
F8: spacing unit and density per surface: 8 px unit, low-medium density, sections at least 48 px apart
F9: imagery role: sparse, directly labelled charts are the imagery; no photography
F10: signature moves: a large figure with a one-sentence reading beside it; deltas with a sign and an arrow; one chart per point
F11: forbidden moves (3-6, testable): no more than 5 figures above the fold; no red or green without a sign or arrow; no unlabelled chart; no decorative icons; no auto-rotating carousel
F12: responsive plan: figures stack one per row by importance; targets 44 px; charts reduce to the key series
F13: state coverage: chart hover details, focus on every figure card, loading skeleton at final size, empty ("no data for this period"), error showing the last good value with its date, disabled, active
F14: fallbacks: system sans with tabular figures as the numeric fallback
F15: motion: a number may count up once in at most 400 ms; nothing else moves; reduced motion shows the final value

## Wayfinder
F1: mood (three words, one-line idea): orienting, glanceable, spatial; the map or timeline is the page
F2: surface polarity: light by default for daytime use with a dark variant; the base layer is muted in both
F3: accent discipline: accent for the current location, the selection and the primary action; categories use a separate labelled set
F4: neutrals: muted base-layer neutrals in 5 steps so routes and events stand out
F5: type roles and sizes per surface: compact sans; place and time labels 13 to 14 px medium; headings 18 to 24 px; tabular numerals for times
F6: shape language: radius 6 to 8 px on drawers and chips; markers differ by shape; sheets round only the top corners
F7: depth strategy: one soft shadow for the floating drawer, tonal steps elsewhere
F8: spacing unit and density per surface: 4 px unit and medium-high density with 16 px drawer padding
F9: imagery role: map tiles, floor plans and timeline bars; photographs only inside detail drawers
F10: signature moves: a detail drawer that keeps the map visible; a filter bar on top; a "now" marker
F11: forbidden moves (3-6, testable): no more than 6 category colours; no colour-only categories; no drawer covering the selection; no unlabelled pins at working zoom; no automatic panning without user action
F12: responsive plan: the drawer becomes a bottom sheet with three snap heights; targets 44 px; the timeline scrolls sideways with sticky labels
F13: state coverage: hover and focus on markers, selected, disabled filter, loading skeleton tiles, empty results with a widen-search action, offline error with a cached view
F14: fallbacks: system sans; if the map style needs a face use the system one; tile styles may not be reproducible, so say so
F15: motion: pan and zoom follow the pointer; drawer 200 ms; route draw 300 ms; reduced motion jumps

## Conversation
F1: mood (three words, one-line idea): attentive, fast, plain; a calm column with the answer in front
F2: surface polarity: follows the system setting, with a neutral canvas
F3: accent discipline: the accent marks the user's own input and the send action only, never the assistant's text
F4: neutrals: 5 neutral steps; assistant text on the canvas, user text on a surface one step lifted
F5: type roles and sizes per surface: text sans 16 to 17 px at line height 1.5, mono 14 to 15 px for code and tool results; no display face; column of 60 to 75 characters
F6: shape language: radius 8 to 12 px on the input and result cards, 6 px on code blocks
F7: depth strategy: tonal steps and one hairline around the input; no shadows
F8: spacing unit and density per surface: 4 px unit, low-medium density, messages 16 to 24 px apart
F9: imagery role: none by default; inline structured results (tables, previews) carry the interest
F10: signature moves: a persistent input docked at the bottom; inline result cards; a history rail
F11: forbidden moves (3-6, testable): no bubbles as the only speaker cue; no avatar galleries; no typing indicator without text; no per-character text animation; no errors hidden in the stream
F12: responsive plan: the history rail becomes a drawer; the input stays above the on-screen keyboard; targets 44 px; result tables scroll in a frame
F13: state coverage: hover on message actions, focus, input disabled while sending, streaming, empty with 3 real example prompts, error with retry, stopped
F14: fallbacks: system sans and mono; name a substitute for any custom text face
F15: motion: text streams without effects; a new message scrolls in 150 ms; reduced motion jumps

## Handbook
F1: mood (three words, one-line idea): practical, scannable, calm; find it, read it, copy it
F2: surface polarity: light by default with an equal dark; code blocks tinted in both
F3: accent discipline: accent for links, the active nav item and the copy action; not for headings
F4: neutrals: 5 neutral steps; the code surface one step off the page
F5: type roles and sizes per surface: text sans 16 px; headings 28, 22 and 18 px in sentence case; mono 14 px
F6: shape language: radius 4 to 6 px, code blocks 6 px; pills only for version tags
F7: depth strategy: hairlines only; no shadows
F8: spacing unit and density per surface: 4 px unit on an 8 px rhythm, medium density, content width 70 to 80 characters
F9: imagery role: diagrams and terminal captures; no stock imagery
F10: signature moves: three columns (nav, content, on this page); anchor links on headings; a copy button on code; a version switch
F11: forbidden moves (3-6, testable): no auto-playing demos; no low-contrast syntax colours; no silent wrapping of code lines; no hidden navigation; no marketing hero inside docs
F12: responsive plan: nav and on-this-page become drawers below about 1024 px; code scrolls in a frame; targets 44 px on touch
F13: state coverage: hover on links, focus on anchors and copy, active nav, copy success, loading search, empty search with suggestions, a not-found page with navigation
F14: fallbacks: system sans and mono; the syntax palette keeps AA contrast in the fallback
F15: motion: minimal; copy confirmation 150 ms; smooth scrolling off under reduced motion
