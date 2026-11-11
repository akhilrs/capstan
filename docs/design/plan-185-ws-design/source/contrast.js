// Computed WCAG contrast of every visible text element on a rendered board.
// Paste as the body of a Playwright evaluate: () => { ...; return result; }
// Backgrounds are composited up the ancestor chain (rgba blended); backdrop blur is not modelled.
(() => {
  const parse = (c) => { const m = c.match(/rgba?\(([^)]+)\)/); if (!m) return [0, 0, 0, 0]; const p = m[1].split(',').map(Number); return [p[0], p[1], p[2], p.length > 3 ? p[3] : 1]; };
  const over = (top, under) => { const a = top[3] + under[3] * (1 - top[3]); if (a === 0) return [0, 0, 0, 0]; return [0, 1, 2].map(i => (top[i] * top[3] + under[i] * under[3] * (1 - top[3])) / a).concat(a); };
  const lum = (c) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); }; return 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2]); };
  const bgOf = (el) => { const chain = []; for (let e = el; e; e = e.parentElement) chain.push(parse(getComputedStyle(e).backgroundColor)); let bg = [255, 255, 255, 1]; for (let i = chain.length - 1; i >= 0; i--) bg = over(chain[i], bg); return bg; };
  const rows = [];
  for (const el of document.querySelectorAll('#out *')) {
    const own = Array.from(el.childNodes).some(n => n.nodeType === 3 && n.textContent.trim());
    if (!own) continue;
    const r = el.getBoundingClientRect(); if (!r.width || !r.height) continue;
    const st = getComputedStyle(el); if (st.visibility === 'hidden' || +st.opacity === 0) continue;
    const bg = bgOf(el); const fg = over(parse(st.color), bg);
    const a = lum(fg), b = lum(bg); const ratio = (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
    const size = parseFloat(st.fontSize), bold = +st.fontWeight >= 700;
    const need = size >= 24 || (size >= 18.66 && bold) ? 3 : 4.5;
    rows.push({ text: el.textContent.trim().slice(0, 40), ratio: Math.round(ratio * 100) / 100, need });
  }
  rows.sort((x, y) => x.ratio - y.ratio);
  return { checked: rows.length, lowest: rows.slice(0, 4), failures: rows.filter(r => r.ratio < r.need) };
})()
