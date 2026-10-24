# Redesign safety checks

Use this for any package that changes an existing screen. Nothing may disappear without a written reason in the design note.

## Before designing: inventory
Take Playwright screenshots of the current screen and list what is on it:
- Links and navigation: every link, menu item, breadcrumb and tab, with its destination.
- Actions: every button, menu action, keyboard shortcut, bulk action and row action.
- Fields: every input, its label, validation, help text, default and required state.
- Headings and landmarks: the heading outline and the page title.
- Data: every column, metric, filter, sort option and export.
- States: empty, loading, error, partial, success, disabled, permission-denied, long-content and offline.
- Content: legal text, notices, tooltips and confirmation steps.
- Behaviour: focus order, deep links and URL parameters, responsive behaviour already present.
- Accessibility: names, roles, alt text, and contrast that must be no worse than before.

## After designing: diff
Compare the new design with the inventory, item by item. Put the result in the design note under `## Kept, moved, changed, removed`:

| Item | Status | Where now or reason |
| --- | --- | --- |
| <inventory item> | kept / moved / changed / removed | <new place, what changed, or why it goes> |

## Rules
- Every inventory item appears in the table exactly once.
- A removal needs a reason in the table and the user's agreement, asked through the PM before the design is approved.
- A change must say what the user can no longer do, or can now do differently.
- Report the table's counts (kept, moved, changed, removed) in your report to the PM.
