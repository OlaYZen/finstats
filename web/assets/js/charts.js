// Hand-rolled SVG charts. Colors follow the entity (Movie is always violet),
// were validated against the card surface (#262626) with the dataviz validator,
// and text never wears a series color.

import { h, s, num, duration, durationExact, dayLabel, dayLabelLong, dayLabelYear, methodLabel, pct } from './dom.js';

export const TYPES = [
  { key: 'Movie', label: 'Movies', color: '#9085e9' },
  { key: 'Episode', label: 'Episodes', color: '#199e70' },
  { key: 'Audio', label: 'Music', color: '#d95926' },
  { key: 'Other', label: 'Other', color: '#3987e5' },
];
export const METHODS = [
  { key: 'DirectPlay', color: '#9085e9' },
  { key: 'DirectStream', color: '#199e70' },
  { key: 'Transcode', color: '#d95926' },
];
const SINGLE = '#9085e9';
const HEAT_EMPTY = '#2f2f2f';
const HEAT_RAMP = ['#3a3358', '#4b3f80', '#5e4ba8', '#7459d0', '#8f6ff0', '#b49dfb'];
const TIME_STEPS = [60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400, 172800, 360000, 720000, 1800000, 3600000];
const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
const DAYS_LONG = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];

// ---------------------------------------------------------------- tooltip
let tipEl = null;
function tipNode() {
  if (!tipEl) {
    tipEl = h('div', { class: 'tooltip', role: 'tooltip', hidden: true });
    document.body.append(tipEl);
    window.addEventListener('scroll', hideTip, { passive: true });
  }
  return tipEl;
}
export function showTip(rect, content) {
  const el = tipNode();
  el.replaceChildren(content);
  el.hidden = false;
  const tw = el.offsetWidth, th = el.offsetHeight;
  let x = rect.left + rect.width / 2 - tw / 2;
  let y = rect.top - th - 8;
  if (y < 8) y = rect.bottom + 8;
  x = Math.max(8, Math.min(x, window.innerWidth - tw - 8));
  el.style.transform = `translate(${Math.round(x)}px, ${Math.round(y)}px)`;
}
export function hideTip() { if (tipEl) tipEl.hidden = true; }

function tipRows(title, rows, total) {
  return h('div', null,
    h('div', { class: 'tooltip-title' }, title),
    total ? h('div', { class: 'tooltip-row' }, h('span', { class: 'tooltip-key' }), h('strong', null, total.value), h('span', null, total.label)) : null,
    rows.map((r) => h('div', { class: 'tooltip-row' },
      h('span', { class: 'tooltip-key', style: { background: r.color } }),
      h('strong', null, r.value), h('span', null, r.label))));
}

// ---------------------------------------------------------------- helpers
function responsive(wrap, draw) {
  let last = 0;
  const ro = new ResizeObserver(() => {
    const w = Math.floor(wrap.clientWidth);
    if (w > 0 && Math.abs(w - last) >= 2) { last = w; draw(w); }
  });
  ro.observe(wrap);
}

function niceTicks(max, count = 4) {
  if (max <= 0) return [0, 1];
  const raw = max / count;
  const mag = Math.pow(10, Math.floor(Math.log10(raw)));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((x) => x >= raw) || 10 * mag;
  const ticks = [];
  for (let v = 0; v < max + step * 0.999; v += step) ticks.push(Math.round(v * 1000) / 1000);
  return ticks;
}

export function legend(items) {
  return h('ul', { class: 'legend' }, items.map((it) =>
    h('li', null, h('span', { class: 'legend-swatch', style: { background: it.color } }), it.label,
      it.value != null ? h('span', { class: 'legend-value mono' }, it.value) : null)));
}

const fmtMetric = (metric, v) => (metric === 'plays' ? num(v) + (v === 1 ? ' play' : ' plays') : duration(v));

// ---------------------------------------------------------------- stacked columns
/** daily: [{date, plays, watch_s, by_type: {Movie:[plays, watch_s], …}}] */
export function columnsChart({ daily, bucket = 'day', metric = 'watch_s' }) {
  const idx = metric === 'plays' ? 0 : 1;
  const rows = daily || [];
  const val = (d, key) => ((d.by_type && d.by_type[key]) || [0, 0])[idx] || 0;
  const total = (d) => (metric === 'plays' ? d.plays : d.watch_s) || 0;
  const max = Math.max(0, ...rows.map(total));
  const used = TYPES.filter((t) => rows.some((d) => val(d, t.key) > 0));

  const wrap = h('div', { class: 'chart', tabindex: rows.length && max > 0 ? 0 : null, role: 'group',
    'aria-label': `${metric === 'plays' ? 'Plays' : 'Watch time'} per ${bucket}. Use left and right arrow keys to read values.` });
  if (!rows.length || max <= 0) {
    wrap.append(h('div', { class: 'chart-empty' }, 'No plays in this range.'));
    return wrap;
  }

  const plot = h('div', { class: 'chart-plot' });
  wrap.append(plot);
  if (used.length >= 2) wrap.append(legend(used));

  const H = 232, M = { l: 44, r: 8, t: 10, b: 24 };
  let active = -1, geom = null, band = null;

  // y scale: time-aware steps for watch time (30m, 1h, 6h…), integers for plays
  let ticks, tickLabel;
  if (metric === 'plays') {
    ticks = [...new Set(niceTicks(max).map((t) => Math.ceil(t)))];
    tickLabel = (t) => num(t);
  } else {
    const step = TIME_STEPS.find((st) => max / st <= 5) || Math.ceil(max / 5 / 3600000) * 3600000;
    ticks = [];
    for (let v = 0; v < max + step * 0.999; v += step) ticks.push(v);
    tickLabel = (t) => (t === 0 ? '0' : step < 3600 ? duration(t) : num(t / 3600) + 'h');
  }
  const top = ticks[ticks.length - 1];

  function draw(w) {
    const pw = w - M.l - M.r, ph = H - M.t - M.b;
    const slot = pw / rows.length;
    const bw = Math.max(1.5, Math.min(24, slot * 0.68));
    const gap = bw >= 5 ? 2 : 1;
    const y = (v) => M.t + ph - (v / top) * ph;
    geom = { slot, pw };

    const svg = s('svg', { width: w, height: H, viewBox: `0 0 ${w} ${H}`, 'aria-hidden': 'true' });
    for (const t of ticks) {
      const ty = Math.round(y(t)) + 0.5;
      svg.append(s('line', { x1: M.l, x2: w - M.r, y1: ty, y2: ty, class: t === 0 ? 'axis-line' : 'grid-line' }));
      svg.append(s('text', { x: M.l - 8, y: ty + 3.5, class: 'tick', 'text-anchor': 'end' }, tickLabel(t)));
    }
    band = s('rect', { class: 'col-band', x: 0, y: M.t, width: Math.max(slot, bw + 4), height: ph, rx: 3, visibility: 'hidden' });
    svg.append(band);

    const every = Math.max(1, Math.ceil(rows.length / Math.max(2, Math.floor(pw / 72))));
    rows.forEach((d, i) => {
      const cx = M.l + slot * i + slot / 2;
      let acc = 0;
      const segs = used.map((t) => ({ t, v: val(d, t.key) })).filter((x) => x.v > 0);
      segs.forEach((seg, k) => {
        const y0 = y(acc); acc += seg.v; const y1 = y(acc);
        const isTop = k === segs.length - 1;
        let hgt = y0 - y1 - (isTop ? 0 : gap);
        if (hgt < 0.75) hgt = 0.75;
        const x = cx - bw / 2, yy = y0 - hgt - (isTop ? 0 : 0);
        const topY = isTop ? y1 : y0 - hgt;
        if (isTop) {
          const r = Math.min(4, bw / 2, Math.max(0, y0 - topY));
          svg.append(s('path', { fill: seg.t.color, d:
            `M${x},${y0} V${topY + r} Q${x},${topY} ${x + r},${topY} H${x + bw - r} Q${x + bw},${topY} ${x + bw},${topY + r} V${y0} Z` }));
        } else {
          svg.append(s('rect', { x, y: yy, width: bw, height: hgt, fill: seg.t.color }));
        }
      });
      if (i % every === 0 && cx + 24 < w) {
        svg.append(s('text', { x: cx, y: H - 6, class: 'tick', 'text-anchor': 'middle' }, dayLabel(d.date)));
      }
    });

    const hit = s('rect', { x: M.l, y: M.t, width: pw, height: ph, fill: 'transparent' });
    hit.addEventListener('pointermove', (e) => {
      const r = hit.getBoundingClientRect();
      setActive(Math.max(0, Math.min(rows.length - 1, Math.floor(((e.clientX - r.left) / r.width) * rows.length))));
    });
    hit.addEventListener('pointerleave', () => { if (document.activeElement !== wrap) setActive(-1); });
    svg.append(hit);
    plot.replaceChildren(svg);
    if (active >= 0) setActive(active);
  }

  function setActive(i) {
    active = i;
    if (!band || !geom) return;
    if (i < 0) { band.setAttribute('visibility', 'hidden'); hideTip(); return; }
    const bwid = Number(band.getAttribute('width'));
    const bx = M.l + geom.slot * i + geom.slot / 2 - bwid / 2;
    band.setAttribute('x', bx);
    band.setAttribute('visibility', 'visible');
    const d = rows[i];
    const pr = plot.getBoundingClientRect();
    const title = (bucket === 'week' ? 'Week of ' : '') + dayLabelLong(d.date);
    const content = tipRows(title,
      used.filter((t) => val(d, t.key) > 0).map((t) => ({ color: t.color, value: fmtMetric(metric, val(d, t.key)), label: t.label })),
      { value: fmtMetric(metric, total(d)), label: 'total' });
    showTip({ left: pr.left + bx, width: bwid, top: pr.top + M.t, bottom: pr.bottom }, content);
  }

  wrap.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault();
      const n = rows.length;
      setActive(active < 0 ? n - 1 : Math.max(0, Math.min(n - 1, active + (e.key === 'ArrowRight' ? 1 : -1))));
    } else if (e.key === 'Escape') setActive(-1);
  });
  wrap.addEventListener('focus', () => { if (active < 0) setActive(rows.length - 1); });
  wrap.addEventListener('blur', () => setActive(-1));

  responsive(wrap, draw);
  return wrap;
}

export function columnsTable({ daily, bucket = 'day' }) {
  const rows = (daily || []).slice().reverse();
  const cell = (d, key) => ((d.by_type && d.by_type[key]) || [0, 0]);
  return h('div', { class: 'table-scroll chart-table' },
    h('table', { class: 'table' },
      h('thead', null, h('tr', null,
        h('th', null, bucket === 'week' ? 'Week of' : 'Date'), h('th', { class: 'r' }, 'Plays'), h('th', { class: 'r' }, 'Watch time'),
        TYPES.map((t) => h('th', { class: 'r' }, t.label)))),
      h('tbody', null, rows.map((d) => h('tr', null,
        h('td', { class: 'mono' }, dayLabelYear(d.date)),
        h('td', { class: 'mono r' }, num(d.plays)),
        h('td', { class: 'mono r', title: durationExact(d.watch_s) }, duration(d.watch_s)),
        TYPES.map((t) => h('td', { class: 'mono r' }, cell(d, t.key)[1] ? duration(cell(d, t.key)[1]) : '–')))))));
}

// ---------------------------------------------------------------- heatmap
export function heatmap({ data, metric = 'plays' }) {
  const grid = (data && data[metric]) || [];
  const other = (data && data[metric === 'plays' ? 'watch_s' : 'plays']) || [];
  const max = Math.max(0, ...grid.flat());
  const wrap = h('div', { class: 'chart heat', tabindex: max > 0 ? 0 : null, role: 'group',
    'aria-label': 'Plays by weekday and hour. Use arrow keys to read values.' });
  if (max <= 0) { wrap.append(h('div', { class: 'chart-empty' }, 'No plays in this range.')); return wrap; }

  const plot = h('div', { class: 'chart-plot' });
  wrap.append(plot, h('div', { class: 'heat-scale' }, h('span', null, 'Less'),
    [HEAT_EMPTY, ...HEAT_RAMP].map((c) => h('span', { class: 'heat-swatch', style: { background: c } })), h('span', null, 'More')));

  const L = 34, T = 4, B = 20, G = 2, CH = 20;
  let cells = [], active = null, cw = 0;
  const color = (v) => (v <= 0 ? HEAT_EMPTY : HEAT_RAMP[Math.min(HEAT_RAMP.length - 1, Math.floor((v / max) * HEAT_RAMP.length - 1e-9))]);
  const hour = (x) => String(x).padStart(2, '0') + ':00';

  function draw(w) {
    cw = (w - L - G * 23) / 24;
    const Hh = T + 7 * CH + 6 * G + B;
    const svg = s('svg', { width: w, height: Hh, viewBox: `0 0 ${w} ${Hh}`, 'aria-hidden': 'true' });
    cells = [];
    for (let d = 0; d < 7; d++) {
      const y = T + d * (CH + G);
      svg.append(s('text', { x: 0, y: y + CH / 2 + 3.5, class: 'tick' }, DAYS[d]));
      cells[d] = [];
      for (let hr = 0; hr < 24; hr++) {
        const v = (grid[d] && grid[d][hr]) || 0;
        const rect = s('rect', { x: L + hr * (cw + G), y, width: Math.max(1, cw), height: CH, rx: 3, fill: color(v), class: 'heat-cell' });
        rect.addEventListener('pointerenter', () => setActive(d, hr));
        cells[d][hr] = rect;
        svg.append(rect);
      }
    }
    for (let hr = 0; hr < 24; hr += 3) {
      svg.append(s('text', { x: L + hr * (cw + G) + cw / 2, y: Hh - 5, class: 'tick', 'text-anchor': 'middle' }, String(hr).padStart(2, '0')));
    }
    svg.addEventListener('pointerleave', () => { if (document.activeElement !== wrap) setActive(null); });
    plot.replaceChildren(svg);
  }

  function setActive(d, hr) {
    if (active) cells[active[0]]?.[active[1]]?.classList.remove('is-active');
    if (d == null) { active = null; hideTip(); return; }
    active = [d, hr];
    const rect = cells[d][hr];
    rect.classList.add('is-active');
    const v = (grid[d] && grid[d][hr]) || 0, o = (other[d] && other[d][hr]) || 0;
    const plays = metric === 'plays' ? v : o, watch = metric === 'plays' ? o : v;
    showTip(rect.getBoundingClientRect(), h('div', null,
      h('div', { class: 'tooltip-title' }, `${DAYS_LONG[d]} ${hour(hr)}–${hour((hr + 1) % 24)}`),
      h('div', { class: 'tooltip-row' }, h('strong', null, num(plays)), h('span', null, plays === 1 ? 'play' : 'plays')),
      h('div', { class: 'tooltip-row' }, h('strong', null, duration(watch)), h('span', null, 'watched'))));
  }

  wrap.addEventListener('keydown', (e) => {
    const mv = { ArrowLeft: [0, -1], ArrowRight: [0, 1], ArrowUp: [-1, 0], ArrowDown: [1, 0] }[e.key];
    if (mv) {
      e.preventDefault();
      const [d, hr] = active || [0, 0];
      setActive(Math.max(0, Math.min(6, d + mv[0])), Math.max(0, Math.min(23, hr + mv[1])));
    } else if (e.key === 'Escape') setActive(null);
  });
  wrap.addEventListener('focus', () => { if (!active) setActive(0, 0); });
  wrap.addEventListener('blur', () => setActive(null));

  responsive(wrap, draw);
  return wrap;
}

export function heatmapTable({ data }) {
  const plays = (data && data.plays) || [];
  return h('div', { class: 'table-scroll chart-table' },
    h('table', { class: 'table table-dense' },
      h('thead', null, h('tr', null, h('th', null, 'Plays'), Array.from({ length: 24 }, (_, i) => h('th', { class: 'r' }, String(i).padStart(2, '0'))))),
      h('tbody', null, DAYS.map((d, i) => h('tr', null, h('th', { scope: 'row' }, d),
        Array.from({ length: 24 }, (_, hr) => h('td', { class: 'mono r' }, num((plays[i] && plays[i][hr]) || 0))))))));
}

// ---------------------------------------------------------------- stacked bar (part-to-whole, ≤ 6 parts)
/** items: [{label, value, color, display}] */
export function stackedBar(items, { ariaLabel = '' } = {}) {
  const total = items.reduce((a, b) => a + (b.value || 0), 0);
  if (total <= 0) return h('div', { class: 'chart-empty' }, 'No plays in this range.');
  const bar = h('div', { class: 'sbar', role: 'group', 'aria-label': ariaLabel },
    items.filter((it) => it.value > 0).map((it) => {
      const seg = h('div', { class: 'sbar-seg', tabindex: 0, style: { flexGrow: String(it.value), background: it.color },
        'aria-label': `${it.label}: ${it.display}, ${pct(it.value / total, 1)}` });
      const show = () => showTip(seg.getBoundingClientRect(), tipRows(it.label, [{ color: it.color, value: it.display, label: pct(it.value / total, 1) }]));
      seg.addEventListener('pointerenter', show);
      seg.addEventListener('focus', show);
      seg.addEventListener('pointerleave', hideTip);
      seg.addEventListener('blur', hideTip);
      return seg;
    }));
  return h('div', { class: 'sbar-wrap' }, bar,
    h('ul', { class: 'legend legend-values' }, items.map((it) => h('li', null,
      h('span', { class: 'legend-swatch', style: { background: it.color } }),
      h('span', { class: 'legend-name' }, it.label),
      h('span', { class: 'legend-value mono' }, it.display),
      h('span', { class: 'legend-pct mono' }, pct(it.value / total, 1))))));
}

export function methodsBar(methods, metric = 'plays') {
  const byName = Object.fromEntries((methods || []).map((m) => [m.name, m]));
  const known = METHODS.map((m) => ({ ...m, row: byName[m.key] })).filter((m) => m.row);
  const items = known.map((m) => ({ label: methodLabel(m.key), color: m.color, value: m.row[metric] || 0, display: fmtMetric(metric, m.row[metric] || 0) }));
  const rest = (methods || []).filter((m) => !METHODS.some((k) => k.key === m.name));
  if (rest.length) {
    const v = rest.reduce((a, b) => a + (b[metric] || 0), 0);
    if (v > 0) items.push({ label: 'Other', color: '#3987e5', value: v, display: fmtMetric(metric, v) });
  }
  return stackedBar(items, { ariaLabel: 'Share of plays by play method' });
}

// ---------------------------------------------------------------- ranked bucket list (it is its own table)
/** buckets: [{name, plays, watch_s}] — one color for every bar: the categories are nominal. */
export function bucketList(buckets, { labelFn = (x) => x, empty = 'Nothing recorded in this range.' } = {}) {
  const rows = buckets || [];
  if (!rows.length) return h('div', { class: 'chart-empty chart-empty-sm' }, empty);
  const max = Math.max(1, ...rows.map((b) => b.plays || 0));
  return h('table', { class: 'buckets' },
    h('thead', { class: 'sr-only' }, h('tr', null, h('th', null, 'Name'), h('th', null, 'Share'), h('th', null, 'Plays'), h('th', null, 'Watch time'))),
    h('tbody', null, rows.map((b) => h('tr', null,
      h('th', { scope: 'row', class: 'bucket-name', title: labelFn(b.name) }, labelFn(b.name)),
      h('td', { class: 'bucket-bar' }, h('span', { class: 'bucket-track' },
        h('span', { class: 'bucket-fill', style: { width: Math.max(1.5, ((b.plays || 0) / max) * 100) + '%', background: SINGLE } }))),
      h('td', { class: 'mono r bucket-plays' }, num(b.plays)),
      h('td', { class: 'mono r bucket-watch', title: durationExact(b.watch_s) }, duration(b.watch_s))))));
}

// ---------------------------------------------------------------- sparkline
export function sparkline(values, { w = 104, hgt = 30 } = {}) {
  const v = (values || []).map((x) => Number(x) || 0);
  if (v.length < 2 || Math.max(...v) <= 0) return null;
  const max = Math.max(...v), pad = 4;
  const x = (i) => pad + (i / (v.length - 1)) * (w - pad * 2);
  const y = (val) => hgt - pad - (val / max) * (hgt - pad * 2);
  const pts = v.map((val, i) => `${x(i).toFixed(1)},${y(val).toFixed(1)}`).join(' ');
  return s('svg', { class: 'spark', width: w, height: hgt, viewBox: `0 0 ${w} ${hgt}`, 'aria-hidden': 'true' },
    s('polyline', { points: pts, fill: 'none', stroke: '#6a5fb0', 'stroke-width': 1.5, 'stroke-linejoin': 'round', 'stroke-linecap': 'round' }),
    s('circle', { cx: x(v.length - 1), cy: y(v[v.length - 1]), r: 3.5, fill: '#a68af9', stroke: '#262626', 'stroke-width': 2 }));
}
