# Capstan landing page

A static page: `index.html`, three stylesheets and one script, with no build step.

- `tokens.css`: the Capstan design-system tokens (source: `design/system/project/tokens.json`), with the self-hosted IBM Plex faces in `fonts/` (SIL OFL 1.1, `fonts/OFL.txt`).
- `components.css`: install bar, agents table, step rows, buttons, code block. Same file as `design/system/project/components/bundle.css`.
- `page.css`: page layout.
- `app.js`: copy-to-clipboard with copied and error states, the light/dark switch (dark first, remembered in `localStorage`), and the dashboard's one update sequence (skipped under reduced motion).

Preview locally from the repository root:

```sh
python3 -m http.server 8000
# open http://localhost:8000/site/
```
