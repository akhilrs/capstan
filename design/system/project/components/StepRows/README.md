# Step rows

Numbered rows with a title and a description, for content that really is a sequence.

- Markup: `ol.cap-steps` with one `li` per step, each holding `.cap-steps__title` and `.cap-steps__text`. The number comes from a CSS counter in mono `accent`; do not type it.
- Three columns on wide screens (number, title, text); below 560px the text drops under the title.
- Rows are divided by a `line` hairline, not boxed. No cards, no icons.
- Only for ordered steps. A list of unordered facts or features is a plain list, without numbers.
