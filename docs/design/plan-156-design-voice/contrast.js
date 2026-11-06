// Computed-style contrast check for the plan-156 design-voice boards. Run in the page (Playwright browser_evaluate):
// for every element with its own visible text, the text colour is composited over the element's background, built by
// compositing every ancestor's background-color from the root down. Returns the lowest ratio and every failure (< 4.5).
() => {
  const parse = (c) => { const m = c.match(/rgba?\(([^)]+)\)/); if (!m) return [0, 0, 0, 0]; const p = m[1].split(',').map(Number); return [p[0], p[1], p[2], p.length > 3 ? p[3] : 1]; };
  const over = (top, under) => { const a = top[3]; return [top[0] * a + under[0] * (1 - a), top[1] * a + under[1] * (1 - a), top[2] * a + under[2] * (1 - a), 1]; };
  const lum = (c) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); }; return 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2]); };
  const ratio = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
  const bgOf = (el) => { const chain = []; for (let e = el; e && e.nodeType === 1; e = e.parentElement) chain.unshift(e); let bg = [255, 255, 255, 1]; for (const e of chain) bg = over(parse(getComputedStyle(e).backgroundColor), bg); return bg; };
  const rows = [];
  for (const el of document.querySelectorAll('body *')) {
    const own = [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
    if (!own) continue;
    const r = el.getBoundingClientRect(); if (!r.width || !r.height) continue;
    const bg = bgOf(el); const fg = over(parse(getComputedStyle(el).color), bg);
    rows.push({ text: el.textContent.trim().slice(0, 40), ratio: Math.round(ratio(fg, bg) * 100) / 100 });
  }
  rows.sort((a, b) => a.ratio - b.ratio);
  return { elements: rows.length, lowest: rows[0], failures: rows.filter((r) => r.ratio < 4.5) };
}
