// DOM + formatting helpers. Everything that comes from the API goes through
// text nodes — never innerHTML. The only markup strings are the static icons below.

const SVG_NS = 'http://www.w3.org/2000/svg';

function append(el, children) {
  for (const c of children) {
    if (c == null || c === false || c === true) continue;
    if (Array.isArray(c)) append(el, c);
    else el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
}

function apply(el, props, isSvg) {
  for (const [k, v] of Object.entries(props || {})) {
    if (k === 'spellcheck') { el.setAttribute('spellcheck', String(!!v)); continue; }
    if (v == null || v === false) continue;
    if (k === 'class') el.setAttribute('class', Array.isArray(v) ? v.filter(Boolean).join(' ') : v);
    else if (k === 'style' && typeof v === 'object') Object.assign(el.style, v);
    else if (k === 'dataset') Object.assign(el.dataset, v);
    else if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2).toLowerCase(), v);
    else if (!isSvg && k in el && !k.startsWith('aria') && k !== 'list' && k !== 'form' && k !== 'role') {
      try { el[k] = v; } catch { el.setAttribute(k, v); }
    } else el.setAttribute(k, v === true ? '' : v);
  }
}

/** h('div', {class: 'x', onClick}, child, [children], 'text') */
export function h(tag, props, ...children) {
  const el = document.createElement(tag);
  apply(el, props, false);
  append(el, children);
  return el;
}

/** Same as h() for SVG elements. */
export function s(tag, props, ...children) {
  const el = document.createElementNS(SVG_NS, tag);
  apply(el, props, true);
  append(el, children);
  return el;
}

export function clear(el) { el.replaceChildren(); return el; }
export function mount(el, ...children) { el.replaceChildren(); append(el, children); return el; }

// ---------------------------------------------------------------- icons
// Static, trusted path data (24×24, stroke). Never mixed with API data.
const ICONS = {
  home: '<path d="M3 10.5 12 3l9 7.5"/><path d="M5 9.5V21h14V9.5"/><path d="M10 21v-6h4v6"/>',
  activity: '<path d="M3 12h4l3-8 4 16 3-8h4"/>',
  users: '<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20c.6-3.6 3.2-5.5 6.5-5.5s5.9 1.9 6.5 5.5"/><path d="M16 4.8a3.5 3.5 0 0 1 0 6.4"/><path d="M18 14.8c2 .8 3.2 2.6 3.6 5.2"/>',
  user: '<circle cx="12" cy="8" r="4"/><path d="M4 21c.7-4 3.8-6 8-6s7.3 2 8 6"/>',
  library: '<path d="M4 4v16"/><path d="M8 4v16"/><path d="m12 6 4.5-1.2 3.6 14.4-4.5 1.2z"/>',
  play: '<path d="M7 4.5v15l12-7.5z"/>',
  pause: '<path d="M8 5v14"/><path d="M16 5v14"/>',
  sliders: '<path d="M4 6h10"/><path d="M18 6h2"/><circle cx="16" cy="6" r="2"/><path d="M4 12h2"/><path d="M10 12h10"/><circle cx="8" cy="12" r="2"/><path d="M4 18h10"/><path d="M18 18h2"/><circle cx="16" cy="18" r="2"/>',
  log: '<path d="M6 3h9l4 4v14H6z"/><path d="M14 3v5h5"/><path d="M9 13h7"/><path d="M9 17h5"/>',
  settings: '<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.8-3.8"/>',
  plus: '<path d="M12 5v14"/><path d="M5 12h14"/>',
  x: '<path d="M6 6l12 12"/><path d="M18 6 6 18"/>',
  check: '<path d="m5 12.5 4.5 4.500L19 7.5"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/>',
  menu: '<path d="M4 7h16"/><path d="M4 12h16"/><path d="M4 17h16"/>',
  arrowUp: '<path d="M12 19V5"/><path d="m6 11 6-6 6 6"/>',
  trendUp: '<path d="m4 16 6-6 4 4 6-7"/><path d="M15 7h5v5"/>',
  trendDown: '<path d="m4 8 6 6 4-4 6 7"/><path d="M15 17h5v-5"/>',
  minus: '<path d="M6 12h12"/>',
  chevronDown: '<path d="m6 9 6 6 6-6"/>',
  chevronRight: '<path d="m9 6 6 6-6 6"/>',
  chevronLeft: '<path d="m15 6-6 6 6 6"/>',
  table: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 10h18"/><path d="M3 15h18"/><path d="M9 4v16"/>',
  chart: '<path d="M4 20V10"/><path d="M10 20V4"/><path d="M16 20v-7"/><path d="M22 20H2"/>',
  upload: '<path d="M12 16V4"/><path d="m7 9 5-5 5 5"/><path d="M4 16v3a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-3"/>',
  alert: '<path d="M12 3 2.5 20h19z"/><path d="M12 10v4.5"/><path d="M12 17.500v.01"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6"/><path d="M12 7.500v.01"/>',
  logout: '<path d="M10 4H5v16h5"/><path d="M14 8l4 4-4 4"/><path d="M18 12H9"/>',
  trash: '<path d="M4 7h16"/><path d="M9 7V4h6v3"/><path d="M6 7l1 13h10l1-13"/>',
  refresh: '<path d="M20 11a8 8 0 0 0-14.5-4"/><path d="M5 3v4h4"/><path d="M4 13a8 8 0 0 0 14.5 4"/><path d="M19 21v-4h-4"/>',
  film: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 4v16"/><path d="M17 4v16"/><path d="M3 9h4"/><path d="M3 15h4"/><path d="M17 9h4"/><path d="M17 15h4"/>',
  database: '<ellipse cx="12" cy="6" rx="8" ry="3"/><path d="M4 6v6c0 1.7 3.6 3 8 3s8-1.3 8-3V6"/><path d="M4 12v6c0 1.7 3.6 3 8 3s8-1.3 8-3v-6"/>',
  link: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.700l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.700l1-1"/>',
  shield: '<path d="M12 3 4.5 6v6c0 4.5 3 7.7 7.5 9 4.5-1.3 7.5-4.5 7.5-9V6z"/>',
  server: '<rect x="3" y="4" width="18" height="7" rx="2"/><rect x="3" y="13" width="18" height="7" rx="2"/><path d="M7 7.5h.01"/><path d="M7 16.500h.01"/>',
  heart: '<path d="M12 20s-7.5-4.6-7.5-10.2A4.3 4.3 0 0 1 12 7.300a4.300 4.300 0 0 1 7.500 2.500C19.500 15.400 12 20 12 20z"/>',
  globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3c2.800 3 2.800 15 0 18"/><path d="M12 3c-2.800 3-2.800 15 0 18"/>',
  lan: '<rect x="9" y="3" width="6" height="5" rx="1"/><rect x="3" y="16" width="6" height="5" rx="1"/><rect x="15" y="16" width="6" height="5" rx="1"/><path d="M12 8v4"/><path d="M6 16v-4h12v4"/>',
  external: '<path d="M14 4h6v6"/><path d="M20 4 11 13"/><path d="M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>',
  skip: '<path d="m4 6 8 6-8 6z"/><path d="m12 6 8 6-8 6z"/>',
  stop: '<rect x="6" y="6" width="12" height="12" rx="1.500"/>',
  volume: '<path d="M4 10v4h3.500L12 18V6l-4.500 4z"/><path d="M15.500 9a4 4 0 0 1 0 6"/><path d="M18 6.500a8 8 0 0 1 0 11"/>',
  captions: '<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M7 14h4"/><path d="M13 14h4"/><path d="M7 10.500h2"/><path d="M11 10.500h6"/>',
  tag: '<path d="M12.586 2.586A2 2 0 0 0 11.172 2H4a2 2 0 0 0-2 2v7.172a2 2 0 0 0 .586 1.414l8.704 8.704a2.426 2.426 0 0 0 3.42 0l6.58-6.58a2.426 2.426 0 0 0 0-3.42z"/><circle cx="7.5" cy="7.5" r="1"/>',
  recap: '<path d="M4 12a8 8 0 1 0 2.600-5.900"/><path d="M4 4v4h4"/><path d="M12 8v4.500l3 1.800"/>',
  cpu: '<rect x="6" y="6" width="12" height="12" rx="2"/><rect x="10" y="10" width="4" height="4"/><path d="M9 3v3"/><path d="M15 3v3"/><path d="M9 18v3"/><path d="M15 18v3"/><path d="M3 9h3"/><path d="M3 15h3"/><path d="M18 9h3"/><path d="M18 15h3"/>',
};

const iconTpl = document.createElement('template');
/** The finstats logo. One source for favicon, sidebar and sign-in: /assets/logo.svg. */
export function logo(size = 24) {
  return h('img', { class: 'brand-logo', src: '/assets/logo.svg', width: size, height: size, alt: '', decoding: 'async' });
}

export function icon(name, size = 16, cls = '') {
  iconTpl.innerHTML =
    `<svg xmlns="${SVG_NS}" viewBox="0 0 24 24" width="${Number(size)}" height="${Number(size)}" fill="none" ` +
    `stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" ` +
    `class="icon">${ICONS[name] || ''}</svg>`;
  const el = iconTpl.content.firstChild;
  if (cls) el.classList.add(...cls.split(' '));
  return el;
}

// ---------------------------------------------------------------- formatting
const nf = new Intl.NumberFormat('en-US');
export const num = (n) => nf.format(Math.round(Number(n) || 0));

export function compact(n) {
  n = Number(n) || 0;
  const abs = Math.abs(n);
  if (abs < 10000) return nf.format(Math.round(n));
  if (abs < 1e6) return trim1(n / 1e3) + 'K';
  if (abs < 1e9) return trim1(n / 1e6) + 'M';
  return trim1(n / 1e9) + 'B';
}
const trim1 = (x) => (Math.abs(x) >= 100 ? Math.round(x).toString() : x.toFixed(1).replace(/\.0$/, ''));

/** 3h 12m · 48m · 2d 4h · 35s */
export function duration(sec) {
  sec = Math.max(0, Math.round(Number(sec) || 0));
  if (sec < 60) return sec + 's';
  const m = Math.floor(sec / 60);
  if (m < 60) return m + 'm';
  const hh = Math.floor(m / 60);
  if (hh < 48) return hh + 'h' + (m % 60 ? ' ' + (m % 60) + 'm' : '');
  const d = Math.floor(hh / 24);
  return d + 'd' + (hh % 24 ? ' ' + (hh % 24) + 'h' : '');
}

export function durationExact(sec) {
  sec = Math.max(0, Math.round(Number(sec) || 0));
  const hh = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60), ss = sec % 60;
  return `${nf.format(hh)}h ${m}m ${ss}s`;
}

/** h:mm:ss clock for playback positions */
export function clock(sec) {
  sec = Math.max(0, Math.round(Number(sec) || 0));
  const hh = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60), ss = sec % 60;
  const p = (x) => String(x).padStart(2, '0');
  return hh ? `${hh}:${p(m)}:${p(ss)}` : `${m}:${p(ss)}`;
}

export function durEl(sec, cls = 'mono') {
  return h('span', { class: cls, title: durationExact(sec) }, duration(sec));
}

export function bytes(n) {
  n = Number(n) || 0;
  if (n < 1024) return n + ' B';
  const u = ['KB', 'MB', 'GB', 'TB', 'PB'];
  let i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < u.length - 1);
  return (n >= 100 ? Math.round(n) : n.toFixed(1)) + ' ' + u[i];
}

export function bitrate(bps) {
  bps = Number(bps) || 0;
  if (!bps) return '–';
  return bps >= 1e6 ? (bps / 1e6).toFixed(1) + ' Mbps' : Math.round(bps / 1e3) + ' kbps';
}

const dtf = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' });
const dayf = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' });
const dayfy = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
const weekdayf = new Intl.DateTimeFormat(undefined, { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric' });

export const dateTime = (ts) => (ts ? dtf.format(new Date(ts * 1000)) : '–');

export function relTime(ts) {
  if (!ts) return 'never';
  const d = Date.now() / 1000 - ts;
  if (d < 45) return 'just now';
  if (d < 3600) return Math.round(d / 60) + 'm ago';
  if (d < 86400) return Math.round(d / 3600) + 'h ago';
  if (d < 86400 * 30) return Math.round(d / 86400) + 'd ago';
  if (d < 86400 * 365) return Math.round(d / (86400 * 30)) + 'mo ago';
  return Math.round(d / (86400 * 365)) + 'y ago';
}

export function relEl(ts, cls = 'mono') {
  return h('time', { class: cls, title: dateTime(ts), dateTime: ts ? new Date(ts * 1000).toISOString() : '' }, relTime(ts));
}

/** 'YYYY-MM-DD' → local Date (no TZ shifting) */
export function parseDay(str) {
  const [y, m, d] = String(str).split('-').map(Number);
  return new Date(y, (m || 1) - 1, d || 1);
}
export const dayLabel = (str) => dayf.format(parseDay(str));
export const dayLabelLong = (str) => weekdayf.format(parseDay(str));
export const dayLabelYear = (str) => dayfy.format(parseDay(str));

export const pct = (x, digits = 0) => (x == null ? '–' : (x * 100).toFixed(digits) + '%');

export function initials(name) {
  const parts = String(name || '?').trim().split(/\s+/).filter(Boolean);
  const a = parts[0]?.[0] || '?';
  const b = parts.length > 1 ? parts[parts.length - 1][0] : '';
  return (a + b).toUpperCase();
}

export function episodeCode(season, episode) {
  if (season == null && episode == null) return '';
  const p = (x) => String(x).padStart(2, '0');
  return (season != null ? 'S' + p(season) : '') + (episode != null ? 'E' + p(episode) : '');
}

export const METHOD_LABEL = { DirectPlay: 'Direct play', DirectStream: 'Direct stream', Transcode: 'Transcode' };
export const methodLabel = (m) => METHOD_LABEL[m] || m || 'Unknown';

/** "ContainerNotSupported" → "Container not supported" */
export function humanize(str) {
  const sp = String(str || '').replace(/([a-z0-9])([A-Z])/g, '$1 $2').replace(/_/g, ' ');
  return sp.charAt(0).toUpperCase() + sp.slice(1).toLowerCase();
}

/** External links come from the API: only https: is ever rendered as a link. */
export function safeHttps(url) {
  try { const u = new URL(String(url)); return u.protocol === 'https:' ? u.href : null; } catch { return null; }
}

/** Time of day, for timelines: 21:04:12 */
const timef = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
export const timeOfDay = (ts) => (ts ? timef.format(new Date(ts * 1000)) : '–');

export function debounce(fn, ms) {
  let t;
  const d = (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); };
  d.cancel = () => clearTimeout(t);
  return d;
}

export const store = {
  get(k, fallback = null) { try { const v = localStorage.getItem(k); return v == null ? fallback : v; } catch { return fallback; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* private mode etc. */ } },
};
