# Design rules

Each rule is one line: an id, then a statement you can judge by looking at the design or by one computed-style check. All numbers are defaults. DESIGN.md or the design system overrides them, and a rule deviation is listed with the reason. A project may replace a rule by id in `docs/design/rules.md`.

## Typography
T1. Pass: at most two typefaces, and each has a stated job (display, text or data); fail: a third face, or a face with no job.
T2. Pass: headings and body differ in more than size (weight, width, case or family); fail: only the size changes.
T3. Pass: reading text is 16 px or more on web (15 px floor for dense product UI), line height 1.4 to 1.6, prose lines 45 to 80 characters, and labels, captions and cells go down to 13 px only when C3 and A5b still pass; fail: anything under 12 px.
T4. Pass: no two adjacent sizes within 1.2x of each other and at most 7 sizes in use; fail: a muddy scale with near-equal steps.
T5. Pass: at most three weights, and bold marks structure, not emphasis inside running text; fail: bold sprinkled through paragraphs.
T6. Pass: all-caps text is one short line and letter-spaced; fail: a long all-caps passage.
T7. Pass: numbers in tables and metrics use tabular figures and align on the decimal point or the right edge; fail: ragged digits.
T8. Pass: a marketing or brand surface uses a deliberate face; fail: the default system or tool font stack is the only face there, unless the design system sets it.

## Colour
C1. Pass: one action accent (primary actions and active state) covers well under 10% of the screen area, data-series colours are not counted as accents, and extra brand colours (Poster, Playground, Zine) appear only when the brief allows, each with a named job and none on actions; fail: a second accent on actions.
C2. Pass: neutrals are tinted toward the brand hue; fail: pure #000 text on a pure #fff surface.
C3. Pass: text contrast is at least 4.5:1 (3:1 for large text, 18 pt or 14 pt bold) and interface boundaries and icons reach 3:1 (WCAG 2.2 AA); fail: any pair below.
C4. Pass: success, warning and error colours differ from the accent and each carries an icon or text as well; fail: colour is the only signal.
C5. Pass: no gradient unless it encodes data or the brief names it; fail: a default blue-to-purple style blend.
C6. Pass: dark mode is designed, with surfaces stepping lighter by elevation, the accent re-tuned for contrast and no large white areas; fail: a plain inversion.
C7. Pass: data-series colours stay distinguishable without hue alone (shape, label or lightness); fail: series told apart by hue only.

## Layout
L1. Pass: one spacing scale (a base unit and its multiples); fail: any value off the scale.
L2. Pass: a clear reading order with one primary element per screen, visible when you squint; fail: several elements compete.
L3. Pass: elements align to a shared grid or a few shared edges; fail: edges drift by a few pixels.
L4. Pass: related items sit closer than unrelated ones by at least 1.5x, grouped by spacing before borders; fail: borders doing the work of spacing.
L5. Pass: at least one place uses asymmetry, a different column ratio or a deliberate break in rhythm when the brief allows; fail: every section is a centred column of equal cards.
L6. Pass: three identical items in a row appear only when the content is truly three parallel items; fail: filler triples.
L7. Pass: containers nest at most two levels unless there is a reason; fail: a box inside a box inside a box.
L8. Pass: the design shows 375 px and 1440 px and the narrow layout is re-composed; fail: the wide layout is merely stacked.
L9. Pass: density matches the brief (data screens are not padded like landing pages, landing pages are not packed like tables); fail: the wrong density for the surface.

## Imagery
I1. Pass: images show the real product, people, place or data of this brief; fail: abstract blobs, floating 3D shapes or generic mesh backgrounds.
I2. Pass: one illustration or icon style per design (same stroke weight, corner treatment and fill logic); fail: mixed styles.
I3. Pass: icons sit beside a label or in a known pattern; fail: icons used as decoration in a grid.
I4. Pass: every image has a role (explain, prove, orient or evoke) written in the proposal; fail: an image with no role.
I5. Pass: crops and aspect ratios are consistent within a set and focus on the subject; fail: mixed ratios or lost subjects.
I6. Pass: placeholder images are marked as placeholders in the notes and replaced by real or licensed ones before build; fail: unmarked placeholders.

## Copy
P1. Pass: all text is real or realistic for this product; fail: lorem ipsum, "Feature One" or similar filler.
P2. Pass: headlines say what the product does or what the user gets, in concrete words; fail: empty phrases such as "Unlock the power of" or "Seamless, intuitive, powerful".
P3. Pass: buttons are verbs naming the result ("Save changes", "Send invoice"); fail: "Submit" or "Click here".
P4. Pass: numbers are specific ("3 of 12 checks passed"); fail: rounded boasts.
P5. Pass: empty, error and loading states have written copy that says what happened and what to do next; fail: a missing or generic message.
P6. Pass: sentence case or title case is chosen once and kept, and the tone matches the brief; fail: mixed cases or a tone clash.

## Motion
M1. Pass: every animation answers where this came from, what changed or what can be done; fail: motion with no such answer.
M2. Pass: durations are 100 to 300 ms for feedback and up to 500 ms for large transitions, with one easing family; fail: slower or mixed easing.
M3. Pass: no looping decorative motion near text; fail: a loop beside reading text.
M4. Pass: a reduced-motion alternative (fade or none) is specified; fail: none stated.
M5. Pass: waits over about one second use a skeleton or progress that matches the final layout; fail: a lone spinner on a blank page.

## Components
K1. Pass: one corner-radius family (for example 0, 4 and 12 px) that fits the direction's mood; fail: seven radii, or everything pill-shaped.
K2. Pass: elevation has one purpose (overlay or lift) and at most three levels; fail: a shadow on every card.
K3. Pass: primary, secondary and tertiary actions look different and each view has one primary action; fail: competing primaries.
K4. Pass: form labels are visible, errors sit beside the field and required fields are marked; fail: placeholder-only labels.
K5. Pass: tables and lists state a row height, sortable and selected states and an overflow plan (truncate, wrap or scroll); fail: an unstated overflow.
K6. Pass: every interactive component shows hover, focus, active, disabled and, where relevant, loading; fail: a missing state.
K7. Pass: the same component looks the same everywhere and variants are named; fail: one-off lookalikes.

## Design record
R1. Pass: every colour, type style, radius and spacing value in the design appears in the token list of the proposal; fail: a value missing from that list.
R2. Pass: every colour has a stated role (brand action, surface, text or state) and two colours sharing a role have a reason; fail: unexplained duplicates.
R3. Pass: the direction names one depth strategy (shadow, tonal steps, hairlines or none) and uses only that; fail: mixed strategies per card.
R4. Pass: the direction lists 3 to 6 forbidden moves and the design breaks none; fail: fewer than 3, or one is broken.
R5. Pass: when marketing and product surfaces both exist, density and type size are stated for each; fail: one averaged value.
R6. Pass: a wanted proprietary or unavailable typeface has a named substitute; fail: no fallback named.
R7. Pass: a rule stated as absolute ("never", "only") has no exception elsewhere in the same record; fail: a contradiction.

## Accessibility
A1. Pass: contrast ratios as in C3 are measured by computed style; fail: judged by eye.
A2. Pass: pointer targets are at least 24 by 24 CSS px (WCAG 2.2 SC 2.5.8) and 44 px or more on touch-first screens, unless an exception applies; fail: smaller targets.
A3. Pass: every control shows a visible focus ring of at least 3:1 against its surroundings and tab order follows reading order; fail: a hidden or low-contrast focus.
A4. Pass: meaning is never carried by colour alone; fail: colour-only meaning.
A5. Pass: content works at 320 CSS px wide without two-dimensional scrolling or loss of function (WCAG 2.2 SC 1.4.10), parts that need two dimensions (data tables, maps, diagrams, games) state an overflow plan consistent with K5; fail: sideways scrolling of ordinary content.
A5b. Pass: text enlarges to 200% without assistive technology and without loss of content or function (SC 1.4.4); fail: clipped or overlapping text.
A6. Pass: headings form a real outline (one h1, no skipped levels) and icon-only controls have an accessible name noted in the design; fail: a skipped level or an unnamed control.
