## Design guidance

This adds to each stage of your prompt and contradicts none of it. Read a topic when a stage below points to it: `cstan design guide <topic>`. Topics: rules, directions-product, directions-brand, redesign, proposal, record.

### Precedence
- DESIGN.md or the design system beats every numeric default here; all numbers in the rules are defaults.
- Where this guidance and the Avoid/Require list in your prompt differ, the Avoid/Require list wins.
- The project may add `docs/design/directions.md` (more directions, same fields as the `proposal` topic) and `docs/design/rules.md` (rule lines). Read both if present. A line `<id>. text` replaces the rule with that id; a new id adds a rule.
- A collection of "like product X" files outside the repository is read only when the PM names its path. Record traits only (density, accent discipline, layout habits), never values or tokens.

### Soft gate
A failed rule never blocks or delays your report. List each failure as a rule deviation with its id and the reason (brief, design system or accepted trade-off). The user decides.

### Brief
Add these to the brief you already write. The reference products record traits only, no values. Before designing, read in this order and note a line each (or "none found"):
1. DESIGN.md at the repository root, if present, first; then `cstan design guide rules`.
2. Design system and token files: Claude Design system README and tokens (DesignSync), CSS variables, theme or token files.
3. Brand files: logo, fonts, colours, voice; note the fixed values.
4. Current UI: Playwright screenshots of the screen and its neighbours; note patterns to reuse.
5. Platform limits: framework, component library, browsers, languages, dark mode, icon set.
6. Content extremes: real strings, longest and shortest values, item counts.
7. Users and tasks: device, frequency, the task that must be fast.
8. Accessibility target: WCAG 2.2 AA unless the project says more.
9. Scope and deadline. Also write three tone words and the four axis scores.

### Directions
- Offer 2 or 3. Always one safe and one stretch, far apart on at least two of the four axes: task intensity (browse to operate), data density, brand expression (neutral to loud), audience (expert to general).
- Score the brief on each axis, then pick from the catalog; blend at most two catalog entries.
- Product entries are in `directions-product`, brand entries in `directions-brand`. Write each direction in the `proposal` template. State the recommendation and the axes on which the directions differ.

### Design package
- The design note gets three more headings: `## Rule deviations`, `## Design record changes`, and, for existing screens, `## Kept, moved, changed, removed` (see `redesign`).
- Write or update DESIGN.md only when the package owns it; see `record`.

### Build
Use the DESIGN.md tokens when it is present. If it has a problem, report it to the PM; never edit it.

### Verify
Use the rules as the self-critique list, next to the Avoid/Require list. When DESIGN.md exists, also run its checklist from `record`.

### Rule index
Each line is a group; `rules` holds the full pass/fail statement for every id.
Typography: T1 two faces; T2 heading vs body; T3 text size and measure; T4 scale ratio; T5 weights; T6 all caps; T7 tabular figures; T8 brand face
Colour: C1 one action accent; C2 tinted neutrals; C3 text contrast; C4 state colours; C5 gradients; C6 designed dark mode; C7 data series
Layout: L1 spacing scale; L2 reading order; L3 alignment; L4 grouping; L5 asymmetry; L6 repeated triples; L7 nesting; L8 responsive; L9 density
Imagery: I1 real subject; I2 one style; I3 icon meaning; I4 image role; I5 crops; I6 placeholders
Copy: P1 real text; P2 concrete headlines; P3 verb buttons; P4 specific numbers; P5 state copy; P6 tone and case
Motion: M1 meaning; M2 duration and easing; M3 no looping near text; M4 reduced motion; M5 loading
Components: K1 radius family; K2 elevation; K3 action hierarchy; K4 forms; K5 tables and lists; K6 states; K7 consistency
Record: R1 token list; R2 colour roles; R3 depth strategy; R4 forbidden moves; R5 per-surface density; R6 substitutes; R7 no contradictions
Accessibility: A1 computed contrast; A2 target size; A3 focus; A4 not colour alone; A5 reflow; A5b resize text; A6 headings and names

### Catalog index
Each line is a direction; the full fields are in `directions-product` or `directions-brand`.
- Ledger: calm, exact, trustworthy. Use: finance, analytics, admin, logs, tables. Avoid: first-impression marketing, emotional products.
- Control Room: alert, focused, undecorated. Use: operations consoles, incidents, builds, monitoring. Avoid: consumer onboarding.
- Workbench: capable, quiet chrome. Use: editors, creator and developer tools, planners. Avoid: content sites.
- Instrument: engineered, exact, live. Use: devices, energy, lab and science tools. Avoid: soft consumer brands.
- Briefing: concise, confident. Use: decision-maker dashboards, digests, reports. Avoid: open-ended exploration.
- Wayfinder: orienting, glanceable. Use: maps, schedules, logistics, calendars. Avoid: static content.
- Conversation: attentive, fast. Use: assistants, support, messaging, command palettes. Avoid: browse-heavy catalogues.
- Handbook: practical, scannable. Use: API docs, CLIs, developer portals, changelogs. Avoid: consumer storytelling.
- Poster: loud, confident, memorable. Use: launches, events, culture, brands with a view. Avoid: long sessions, dense data.
- Quiet Luxury: restrained, sparse, slow. Use: premium goods, hospitality, portfolios, wealth. Avoid: urgent or utility flows.
- Storefront: clear, appetising, quick. Use: shops, marketplaces, catalogues, booking. Avoid: back-office tools.
- Playground: warm, encouraging, playful. Use: education, family, wellness, community. Avoid: finance, safety-critical flows.
- Zine: handmade, opinionated, expressive. Use: creative communities, music, independent media. Avoid: regulated or high-trust services.
- Field Notes: thoughtful, human, reading first. Use: docs, blogs, knowledge bases, reports. Avoid: tool-heavy dashboards.
- Plain Speech: clear, respectful, low drama. Use: public services, health, forms, broad audiences. Avoid: brand-led campaigns.
- Throwback: nostalgic, tactile, knowing. Use: games, retro products, era communities. Avoid: speed, reading comfort or access as the main job.
