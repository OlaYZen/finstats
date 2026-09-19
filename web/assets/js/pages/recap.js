// Recap — the year in review. The one loud page in an otherwise quiet app. It opens on the year
// "replayed" as a waveform, a bar per week, beside the posters that filled it; then one idea per
// chapter — a headline with a single accented word, a sentence that carries the figures, the
// chapter's word standing faint behind it — and it ends the way films do: with the credits.

import { h, icon, mount, num, duration, durationExact, pct, parseDay } from '../dom.js';
import { api, imgItem } from '../api.js';
import { state, isAdmin, userList } from '../state.js';
import { replaceQuery } from '../router.js';
import { dataView, segmented, poster, emptyState, sk, combobox } from '../components.js';
import { showTip, hideTip } from '../charts.js';

const BAR = '#9085e9';      // the single series hue used everywhere else in finstats
const BAR_PEAK = '#b49dfb'; // emphasis: the extreme, nothing else

// ---------------------------------------------------------------- formatting
const wdF = new Intl.DateTimeFormat(undefined, { weekday: 'long' });
const dF = new Intl.DateTimeFormat(undefined, { day: 'numeric' });
const mF = new Intl.DateTimeFormat(undefined, { month: 'long' });
const hmF = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });
const mShortF = new Intl.DateTimeFormat(undefined, { month: 'short' });
const WEEKDAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];

/** "Saturday 14 March" */
const longDate = (d) => `${wdF.format(d)} ${dF.format(d)} ${mF.format(d)}`;
const longDay = (str) => longDate(parseDay(str));
const dayMonth = (d) => `${dF.format(d)} ${mF.format(d)}`;
/** "1–9 February" or "28 January – 3 February" */
function dayRange(from, to) {
  const a = parseDay(from), b = parseDay(to);
  if (from === to) return dayMonth(a);
  return a.getMonth() === b.getMonth() && a.getFullYear() === b.getFullYear() ? `${dF.format(a)}–${dF.format(b)} ${mF.format(b)}` : `${dayMonth(a)} – ${dayMonth(b)}`;
}
const monthDate = (ym) => { const [y, m] = String(ym).split('-').map(Number); return new Date(y, (m || 1) - 1, 1); };
const hoursOf = (sec) => (Number(sec) || 0) / 3600;
const hoursText = (sec) => { const x = hoursOf(sec); return x >= 10 ? num(x) : (Math.round(x * 10) / 10).toLocaleString(); };
const plural = (n, one, many) => `${num(n)} ${Number(n) === 1 ? one : many}`;
const hour2 = (i) => String(i).padStart(2, '0') + ':00';

// ---------------------------------------------------------------- voice
/** Your own recap speaks to you; an administrator looking at someone else's reads about them by name. */
function voiceFor(scope, me) {
  if (!scope || !scope.user_id || (me && scope.user_id === me.id)) return { you: true, who: 'You', whoLow: 'you', your: 'Your', yourLow: 'your' };
  const name = scope.user_name || 'This user';
  return { you: false, who: name, whoLow: name, your: `${name}’s`, yourLow: `${name}’s` };
}

// ---------------------------------------------------------------- motion
const reducedMotion = () => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;

/** A number that counts up once, the first time its chapter scrolls into view. */
function countUp(value, format = num) {
  const el = h('span', { class: 'rc-count' }, format(value));
  el._count = { value: Number(value) || 0, format };
  return el;
}
function runCount(el) {
  const c = el._count;
  if (!c || c.done) return;
  c.done = true;
  if (reducedMotion() || c.value <= 0) return;
  const t0 = performance.now(), ms = 900;
  const step = (t) => {
    const k = Math.max(0, Math.min(1, (t - t0) / ms));
    el.textContent = c.format(c.value * (1 - Math.pow(1 - k, 3)));
    if (k < 1 && el.isConnected) requestAnimationFrame(step); else el.textContent = c.format(c.value);
  };
  el.textContent = c.format(0);
  requestAnimationFrame(step);
}

/** Chapters fade up as they arrive; their direct children follow in a short stagger. */
function revealer() {
  const show = (el) => { el.classList.add('is-in'); el.querySelectorAll('.rc-count').forEach(runCount); };
  if (typeof IntersectionObserver !== 'function' || reducedMotion()) return { animated: false, watch: show, stop() {} };
  const io = new IntersectionObserver((entries) => {
    for (const e of entries) if (e.isIntersecting) { show(e.target); io.unobserve(e.target); }
  }, { threshold: 0.12, rootMargin: '0px 0px -6% 0px' });
  return { animated: true, watch: (el) => io.observe(el), stop: () => io.disconnect() };
}

// ---------------------------------------------------------------- small pieces
const em = (...t) => h('em', { class: 'rc-accent' }, ...t);
const b = (...t) => h('strong', null, ...t);
const tail = (...t) => h('i', { class: 'rc-tail' }, ...t);

/** [{value: Node|string, label}] → big figures over small capitals. */
function statRow(stats) {
  const rows = (stats || []).filter(Boolean);
  if (!rows.length) return null;
  return h('dl', { class: 'rc-stats' }, rows.map((x) => h('div', null, h('dd', null, x.value), h('dt', null, x.label))));
}

/**
 * One chapter: icon + eyebrow, headline, a ruled lead sentence, optional figures, then the body.
 * `mark` is the chapter's word, set huge and faint behind everything. Decoration only.
 */
function chapter({ id, icon: ico, eyebrow, title, lead, stats, body, mark, cls = '', label }) {
  const kids = [
    eyebrow ? h('p', { class: 'rc-eyebrow' }, ico ? icon(ico, 15) : null, h('span', null, eyebrow)) : null,
    title ? h('h2', { class: 'rc-title' }, title) : null,
    lead ? h('p', { class: 'rc-lead' }, lead) : null,
    statRow(stats),
    ...(Array.isArray(body) ? body : [body]),
  ].filter(Boolean);
  kids.forEach((k, i) => k.style.setProperty('--i', String(i)));
  return h('section', { class: 'rc-chapter rc-reveal ' + cls, id: id ? 'rc-' + id : null, 'aria-label': label || eyebrow || null },
    mark ? h('span', { class: 'rc-mark', 'aria-hidden': 'true' }, String(mark)) : null, kids);
}

const titleLink = (t, cls = '') => (t.id && t.item_exists !== false
  ? h('a', { class: 'rc-link ' + cls, href: `/items/${t.id}` }, t.name || 'Unknown title')
  : h('span', { class: cls }, t.name || 'Unknown title'));

/** The #1 of a list: poster bleeding large beside the type. */
function feature(t, facts) {
  const bleed = t.image_item_id ? h('img', { class: 'rc-bleed', src: imgItem(t.image_item_id, 300), alt: '', 'aria-hidden': 'true', decoding: 'async', onError: (e) => e.target.remove() }) : null;
  return h('div', { class: 'rc-feature' }, bleed,
    poster(t.image_item_id, t.name, { w: 480, cls: 'rc-feature-poster' }),
    h('div', { class: 'rc-feature-text' },
      h('span', { class: 'rc-rank-big mono' }, '#1'),
      h('h3', { class: 'rc-feature-name' }, titleLink(t)),
      t.sub ? h('p', { class: 'rc-feature-sub' }, t.sub) : null,
      h('dl', { class: 'rc-facts' }, facts.filter(Boolean).map(([k, v]) => h('div', null, h('dt', null, k), h('dd', { class: 'mono' }, v))))));
}

function rankedList(rows, start = 2, extra = () => null) {
  if (!rows.length) return null;
  return h('ol', { class: 'rc-ranked', start }, rows.map((t, i) => h('li', { class: 'rc-ranked-row' },
    h('span', { class: 'rc-ranked-n mono' }, String(start + i)),
    poster(t.image_item_id, t.name, { w: 120, cls: 'poster-sm' }),
    h('div', { class: 'rc-ranked-main' }, titleLink(t, 'rc-ranked-name'), h('span', { class: 'rc-ranked-sub' }, [t.sub, extra(t)].filter(Boolean).join(' · ') || ' ')),
    h('span', { class: 'rc-ranked-val mono', title: durationExact(t.watch_s) }, duration(t.watch_s)))));
}

/** Ranked horizontal bars, one hue, value at the end. */
function rankBars(rows, valueOf, format) {
  const max = Math.max(1, ...rows.map(valueOf));
  return h('div', { class: 'rc-rankbars', role: 'list' }, rows.map((r) => h('div', { class: 'rc-rankbar', role: 'listitem' },
    h('span', { class: 'rc-rankbar-name' }, r.name || 'Unknown'),
    h('span', { class: 'rc-rankbar-track' }, h('span', { class: 'rc-rankbar-fill', style: { width: Math.max(1, (valueOf(r) / max) * 100) + '%' } })),
    h('span', { class: 'rc-rankbar-val mono' }, format(r)))));
}

/**
 * Thin columns from one baseline. One tab stop; ←/→ walk the columns and show the same
 * tooltip as hover. Only the peak carries a direct label; a visually hidden table holds every value.
 * rows: [{label, title, value, foot?: Node}]
 */
function barStrip({ rows, format, ariaLabel, head, cls = '', labelEvery = 1 }) {
  const max = Math.max(0, ...rows.map((r) => Number(r.value) || 0));
  const peak = rows.findIndex((r) => (Number(r.value) || 0) === max);
  let active = -1;
  const cols = rows.map((r, i) => {
    const v = Number(r.value) || 0;
    const bar = h('span', { class: 'rc-bar', style: { height: (max > 0 ? Math.max(v > 0 ? 1.5 : 0, (v / max) * 100) : 0) + '%', background: i === peak && max > 0 ? BAR_PEAK : BAR } });
    return h('div', { class: 'rc-col', onPointerenter: () => setActive(i) },
      h('div', { class: 'rc-col-track' }, i === peak && max > 0 ? h('span', { class: 'rc-col-peak mono' }, format(v)) : null, bar),
      h('span', { class: 'rc-col-label mono' }, i % labelEvery === 0 ? r.label : ' '),
      r.foot || null);
  });
  const wrap = h('div', { class: 'rc-bars ' + cls, tabindex: max > 0 ? 0 : null, role: 'group', 'aria-label': `${ariaLabel}. Use left and right arrow keys to read values.` },
    h('div', { class: 'rc-cols', 'aria-hidden': 'true' }, cols),
    h('table', { class: 'sr-only' }, h('caption', null, ariaLabel),
      h('thead', null, h('tr', null, h('th', null, head[0]), h('th', null, head[1]))),
      h('tbody', null, rows.map((r) => h('tr', null, h('td', null, r.title || r.label), h('td', null, format(Number(r.value) || 0)))))));
  function setActive(i) {
    if (active >= 0 && cols[active]) cols[active].classList.remove('is-active');
    active = i;
    if (i < 0) { hideTip(); return; }
    cols[i].classList.add('is-active');
    const r = rows[i];
    showTip(cols[i].querySelector('.rc-col-track').getBoundingClientRect(), h('div', null,
      h('div', { class: 'tooltip-title' }, r.title || r.label),
      h('div', { class: 'tooltip-row' }, h('span', { class: 'tooltip-key', style: { background: BAR } }), h('strong', null, format(Number(r.value) || 0)), r.note ? h('span', null, r.note) : null)));
  }
  wrap.addEventListener('pointerleave', () => { if (document.activeElement !== wrap) setActive(-1); });
  wrap.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault();
      setActive(active < 0 ? Math.max(0, peak) : Math.max(0, Math.min(rows.length - 1, active + (e.key === 'ArrowRight' ? 1 : -1))));
    } else if (e.key === 'Escape') setActive(-1);
  });
  wrap.addEventListener('focus', () => { if (active < 0) setActive(Math.max(0, peak)); });
  wrap.addEventListener('blur', () => setActive(-1));
  return wrap;
}

function recordCard({ label, value, context, image, name }) {
  return h('article', { class: 'rc-record' },
    image !== undefined ? poster(image, name, { w: 300, cls: 'rc-record-poster' }) : null,
    h('div', { class: 'rc-record-text' },
      h('p', { class: 'rc-record-label' }, label),
      h('p', { class: 'rc-record-value' }, value),
      context ? h('p', { class: 'rc-record-context' }, context) : null));
}

// ---------------------------------------------------------------- chapters
/** The period picker: the same segmented control as every other range switch in the app. */
function yearTabs(d, current, onPick) {
  let years = (d.years || []).map(String);
  if (/^\d{4}$/.test(current) && !years.includes(current)) years.push(current);
  years = [...new Set(years)].sort();
  return h('div', { class: 'rc-years' }, segmented({ label: 'Period', value: current,
    options: [...years.map((y) => ({ value: y, label: y })), { value: 'last12', label: 'Last 12 months' }], onChange: onPick }));
}

/** The period cut into Monday-to-Sunday weeks: [{from, to, sec}]. */
function weeksOf(d) {
  const months = (d.months || []).filter((m) => m && m.month);
  if (!months.length) return [];
  const first = monthDate(months[0].month), lastM = monthDate(months[months.length - 1].month);
  const last = new Date(lastM.getFullYear(), lastM.getMonth() + 1, 0);
  const byDay = new Map((d.days || []).map((x) => [x.date, x.watch_s || 0]));
  const out = [];
  let cur = null;
  for (let dt = new Date(first); dt <= last; dt.setDate(dt.getDate() + 1)) {
    if (!cur || (dt.getDay() + 6) % 7 === 0) { cur = { from: new Date(dt), to: new Date(dt), sec: 0, month: null }; out.push(cur); }
    if (dt.getDate() === 1) cur.month = mShortF.format(dt);
    cur.to = new Date(dt);
    cur.sec += byDay.get(isoDay(dt)) || 0;
  }
  return out;
}

/** The year as sound: one bar a week, mirrored around the middle like a waveform. The loudest week is lit. */
function waveform(d) {
  const weeks = weeksOf(d);
  const max = Math.max(0, ...weeks.map((w) => w.sec));
  if (!weeks.length || max <= 0) return null;
  const peak = weeks.findIndex((w) => w.sec === max);
  const range = (w) => dayRange(isoDay(w.from), isoDay(w.to));
  const bars = weeks.map((w, i) => {
    const bar = h('span', { class: 'rc-wave-bar' + (i === peak ? ' is-peak' : '') + (w.sec > 0 ? '' : ' is-silent'), style: { height: (w.sec > 0 ? Math.max(4, (w.sec / max) * 100) : 0) + '%' } });
    bar.style.setProperty('--i', String(i));
    const slot = h('span', { class: 'rc-wave-slot', onPointerenter: () => showTip(slot.getBoundingClientRect(), h('div', null,
      h('div', { class: 'tooltip-title' }, range(w)),
      h('div', { class: 'tooltip-row' }, h('span', { class: 'tooltip-key', style: { background: i === peak ? BAR_PEAK : BAR } }), w.sec > 0 ? h('strong', null, duration(w.sec)) : h('span', null, 'Nothing played')))) }, bar);
    return slot;
  });
  const cols = { gridTemplateColumns: `repeat(${weeks.length}, minmax(0, 1fr))` };
  let lastAt = -9;
  const labels = weeks.map((w, i) => { if (!w.month || i - lastAt < 3) return null; lastAt = i; return h('span', { class: 'mono', style: { gridColumn: String(i + 1) } }, w.month); });
  // Decoration with a caption: the calendar further down is the version that can be read day by day.
  return h('figure', { class: 'rc-wave-wrap' },
    h('div', { class: 'rc-wave', style: cols, 'aria-hidden': 'true', onPointerleave: hideTip }, bars),
    h('div', { class: 'rc-wave-months', style: cols, 'aria-hidden': 'true' }, labels),
    h('figcaption', { class: 'rc-note' }, 'One bar a week. The loudest: ', h('strong', null, range(weeks[peak])), `, with ${duration(max)}.`));
}

function heroChapter(d, v, periodLabel, picker) {
  const t = d.totals || {};
  const isYear = /^\d{4}$/.test(periodLabel);
  const hrs = hoursOf(t.watch_s);
  const r = d.rank;
  const fan = [...(d.top_series || []), ...(d.top_movies || [])].filter((x) => x && x.image_item_id).sort((a, c) => (c.watch_s || 0) - (a.watch_s || 0)).slice(0, 5);
  return h('header', { class: 'rc-hero rc-reveal' },
    picker,
    h('div', { class: 'rc-hero-main' },
      h('div', { class: 'rc-hero-text' },
        h('p', { class: 'rc-eyebrow' }, icon('recap', 15), h('span', null, 'The year in review')),
        h('h2', { class: 'rc-hero-title' }, isYear ? [`${v.your} `, em(periodLabel), ', replayed'] : [`${v.your} last `, em('12 months'), ', replayed']),
        h('p', { class: 'rc-hero-line' },
          b(countUp(hrs, (x) => (hrs >= 10 ? num(x) : (Math.round(x * 10) / 10).toLocaleString())), hrs === 1 ? ' hour' : ' hours'),
          ' across ', b(plural(t.plays, 'play', 'plays')), ' and ', b(plural(t.distinct_items, 'title', 'titles')), '.'),
        r && r.position && r.of > 1 ? h('p', { class: 'rc-hero-rank mono' }, `#${num(r.position)} of ${plural(r.of, 'viewer', 'viewers')}${r.share != null ? ` · ${pct(r.share)} of all watching` : ''}`) : null),
      fan.length >= 3 ? h('div', { class: 'rc-fan', 'aria-hidden': 'true' }, fan.map((x) => h('img', { src: imgItem(x.image_item_id, 300), alt: '', decoding: 'async', onError: (e) => e.target.remove() }))) : null),
    waveform(d));
}

/** A square tile per figure. The icon is only a signpost; the number does the talking. */
function tile(ico, value, label, title) {
  return h('div', { class: 'rc-tile', title: title || null }, h('span', { class: 'rc-tile-icon', 'aria-hidden': 'true' }, icon(ico, 18)),
    h('span', { class: 'rc-tile-num' }, value), h('span', { class: 'rc-tile-label' }, label));
}

function numbersChapter(d, v, periodLabel) {
  const t = d.totals || {};
  const hrs = hoursOf(t.watch_s), days = hrs / 24;
  const daysText = days >= 10 ? num(days) : (Math.round(days * 10) / 10).toLocaleString();
  const streak = d.records && d.records.longest_streak;
  const rw = d.rewatch;
  const split = [
    { name: 'Episodes', watch_s: t.episode_watch_s || 0, plays: t.episodes || 0 },
    { name: 'Movies', watch_s: t.movie_watch_s || 0, plays: t.movies || 0 },
    { name: 'Music', watch_s: t.track_watch_s || 0, plays: t.tracks || 0 },
  ].filter((x) => x.plays > 0).sort((a, c) => c.watch_s - a.watch_s);
  return chapter({
    id: 'numbers', icon: 'chart', eyebrow: 'By the numbers', mark: /^\d{4}$/.test(periodLabel) ? periodLabel : '365',
    title: [`${v.your} year by the `, em('numbers')],
    lead: [`${v.who} pressed play `, b(plural(t.plays, 'time', 'times')), ' on ', b(plural(t.distinct_items, 'different title', 'different titles')),
      hrs >= 24 ? tail(` — that’s ${daysText} ${daysText === '1' ? 'day' : 'days'} without a break.`) : '.'],
    body: [
      h('div', { class: 'rc-tiles' },
        tile('play', countUp(t.plays), 'Plays'),
        tile('clock', countUp(hrs, (x) => `${hrs >= 10 ? num(x) : (Math.round(x * 10) / 10).toLocaleString()}h`), 'Watch time', durationExact(t.watch_s)),
        tile('layers', countUp(t.distinct_items), 'Different titles'),
        tile('calendar', countUp(t.active_days), 'Days watched'),
        tile('flame', streak && streak.days > 1 ? countUp(streak.days) : '—', 'Longest streak, days', streak && streak.days > 1 ? dayRange(streak.from, streak.to) : null),
        tile('repeat', rw ? countUp((rw.share || 0) * 100, (x) => `${Math.round(x)}%`) : '—', 'Rewatches', rw ? `${plural(rw.rewatches, 'sitting', 'sittings')} with something ${v.you ? 'you had' : 'they had'} already seen` : null)),
      split.length > 1 ? h('div', { class: 'rc-split' },
        h('h3', { class: 'rc-subhead' }, 'What it was made of'),
        rankBars(split, (x) => x.watch_s, (x) => `${duration(x.watch_s)} · ${plural(x.plays, 'play', 'plays')}`)) : null,
    ],
  });
}

function topChapter(id, rows, { icon: ico, eyebrow, title, lead, facts, extra, mark }) {
  if (!rows || !rows.length) return null;
  const [first, ...rest] = rows;
  return chapter({ id, icon: ico, eyebrow, mark, title: title(first), lead: lead(first), body: h('div', { class: 'rc-top' }, feature(first, facts(first)), rankedList(rest, 2, extra)) });
}

function genresChapter(d, v) {
  const rows = d.top_genres || [];
  if (!rows.length) return null;
  const top = rows[0], g = d.genres || {};
  const share = g.watch_s > 0 ? top.watch_s / g.watch_s : null;
  const an = /^[aeiou]/i.test(top.name) ? 'an' : 'a';
  return chapter({
    id: 'genres', icon: 'tag', eyebrow: 'Genres', mark: top.name,
    title: v.you ? [`You’re ${an} `, em(top.name), ' fan'] : [`${v.who} is ${an} `, em(top.name), ' fan'],
    lead: [b(top.name), ' led with ', b(duration(top.watch_s)), ' across ', b(plural(top.plays, 'play', 'plays')),
      rows[1] ? tail(` — with ${rows[1].name} close behind.`) : '.'],
    stats: [
      g.count ? { value: countUp(g.count), label: g.count === 1 ? 'Genre' : 'Genres' } : null,
      share != null ? { value: countUp(share * 100, (x) => `${Math.round(x)}%`), label: `Was ${top.name}` } : null,
      { value: countUp(top.plays), label: 'Plays' },
    ],
    body: [rankBars(rows, (x) => x.watch_s || 0, (x) => duration(x.watch_s)),
      h('p', { class: 'rc-note' }, 'A title counts towards each of its genres, and a show counts episode by episode.')],
  });
}

function peopleChapter(d, v) {
  const p = d.people || {};
  const sets = [['actors', 'Actors', p.actors || []], ['directors', 'Directors', p.directors || []]].filter((x) => x[2].length);
  if (!sets.length) return null;
  let tab = sets[0][0];
  const title = h('h2', { class: 'rc-title' }), lead = h('p', { class: 'rc-lead' }), grid = h('ol', { class: 'rc-people' });
  function paint() {
    const rows = sets.find((x) => x[0] === tab)[2], top = rows[0];
    mount(title, tab === 'actors' ? [em(top.name), ' was everywhere'] : [em(top.name), ' called the shots']);
    mount(lead, [b(duration(top.watch_s)), tab === 'actors' ? ' on screen across ' : ' directed across ', b(plural(top.titles, 'title', 'titles')),
      top.top_title ? tail(` — most of it in ${top.top_title}.`) : '.']);
    mount(grid, rows.map((x, i) => h('li', null, h('a', { class: 'rc-person', href: `/people/${x.id}` },
      h('span', { class: 'rc-person-n mono', 'aria-hidden': 'true' }, '.' + String(i + 1).padStart(2, '0')),
      poster(x.has_image ? x.id : null, x.name, { w: 300, cls: 'rc-person-photo' }),
      h('span', { class: 'rc-person-name' }, x.name),
      h('span', { class: 'rc-person-sub mono', title: durationExact(x.watch_s) }, `${duration(x.watch_s)} · ${plural(x.titles, 'title', 'titles')}`)))));
  }
  paint();
  const el = chapter({
    id: 'people', icon: 'users', eyebrow: 'Most watched people', mark: 'Cast',
    body: [title, lead,
      sets.length > 1 ? segmented({ label: 'Who', value: tab, options: sets.map(([value, label]) => ({ value, label })), onChange: (val) => { tab = val; paint(); } }) : null,
      grid],
  });
  return el;
}

function personaChapter(d, v) {
  const p = d.persona;
  if (!p) return null;
  const name = p.title || 'Creature of habit';
  const an = /^[aeiou]/i.test(name) ? 'an' : 'a';
  return chapter({
    id: 'persona', icon: 'sparkle', eyebrow: 'Viewing personality', mark: name, cls: 'rc-persona',
    title: v.you ? [`You’re ${an} `, em(name.toLowerCase())] : [`${v.who} is ${an} `, em(name.toLowerCase())],
    lead: p.line ? [p.line, '.'] : null,
  });
}

/** Hours, weekdays and months share one chapter; the headline follows whichever is showing. */
function rhythmChapter(d, v) {
  const hours = Array.isArray(d.hours) && d.hours.length === 24 && d.hours.some((x) => x > 0) ? d.hours : null;
  const weekdays = Array.isArray(d.weekdays) && d.weekdays.length === 7 && d.weekdays.some((x) => x > 0) ? d.weekdays : null;
  const months = (d.months || []).filter((m) => m && m.month);
  const hasMonths = months.some((m) => m.watch_s > 0);
  const tabs = [hours && ['hours', 'Hours'], weekdays && ['days', 'Days'], hasMonths && ['months', 'Months']].filter(Boolean);
  if (!tabs.length) return null;
  const total = (d.totals && d.totals.watch_s) || 0;
  const peakOf = (arr) => arr.reduce((best, x, i) => (x > arr[best] ? i : best), 0);
  let tab = weekdays ? 'days' : tabs[0][0];
  const title = h('h2', { class: 'rc-title' }), lead = h('p', { class: 'rc-lead' }), plot = h('div', { class: 'rc-plot' });

  function paint() {
    hideTip();
    if (tab === 'hours') {
      const i = peakOf(hours);
      mount(title, [em(hour2(i)), v.you ? ' is your prime time' : ` is ${v.your} prime time`]);
      mount(lead, [b(duration(hours[i])), ` started between ${hour2(i)} and ${hour2((i + 1) % 24)}`, total ? tail(` — ${pct(hours[i] / total)} of everything, in one hour of the day.`) : '.']);
      mount(plot, barStrip({ rows: hours.map((x, k) => ({ label: String(k).padStart(2, '0'), title: `${hour2(k)} – ${hour2((k + 1) % 24)}`, value: x })), format: duration, ariaLabel: 'Watch time by hour of day', head: ['Hour', 'Watch time'], labelEvery: 3, cls: 'rc-bars-24' }));
    } else if (tab === 'days') {
      const i = peakOf(weekdays), avg = weekdays.reduce((a, x) => a + x, 0) / 7;
      const over = avg > 0 ? weekdays[i] / avg - 1 : 0;
      mount(title, [em(WEEKDAYS[i]), ' took the crown']);
      mount(lead, [b(duration(weekdays[i])), ` of watching landed on ${WEEKDAYS[i]}s`, over >= 0.05 ? tail(` — ${pct(over)} more than an average day.`) : '.']);
      mount(plot, barStrip({ rows: weekdays.map((x, k) => ({ label: WEEKDAYS[k].slice(0, 3), title: WEEKDAYS[k], value: x })), format: duration, ariaLabel: 'Watch time by weekday', head: ['Weekday', 'Watch time'], cls: 'rc-bars-7' }));
    } else {
      const spansYears = new Set(months.map((m) => String(m.month).slice(0, 4))).size > 1;
      const best = months.reduce((a, c) => (c.watch_s > a.watch_s ? c : a));
      const rows = months.map((m) => {
        const dt = monthDate(m.month), name = m.top && m.top.name;
        return { label: mShortF.format(dt), title: `${mF.format(dt)}${spansYears ? ' ' + dt.getFullYear() : ''}`, value: m.watch_s, note: name ? `mostly ${name}` : null,
          foot: h('span', { class: 'rc-col-foot', title: name || null }, m.top ? [poster(m.top.image_item_id, name, { w: 120, cls: 'poster-xs' }), h('span', { class: 'rc-col-foot-name' }, name)] : null) };
      });
      mount(title, [em(mF.format(monthDate(best.month))), ' was the big one']);
      mount(lead, [b(duration(best.watch_s)), ' in a single month', best.top && best.top.name ? tail(` — most of it with ${best.top.name}.`) : '.']);
      mount(plot, [barStrip({ rows, format: duration, ariaLabel: 'Watch time per month', head: ['Month', 'Watch time'], cls: 'rc-bars-months' }),
        h('table', { class: 'sr-only' }, h('caption', null, 'Most watched title per month'),
          h('tbody', null, rows.map((r, k) => h('tr', null, h('td', null, r.title), h('td', null, (months[k].top && months[k].top.name) || 'Nothing')))))]);
    }
  }
  paint();
  return chapter({
    id: 'rhythm', icon: 'activity', eyebrow: 'Activity patterns', mark: 'When',
    body: [title, lead,
      tabs.length > 1 ? segmented({ label: 'Show watch time by', value: tab, options: tabs.map(([value, label]) => ({ value, label })), onChange: (val) => { tab = val; paint(); } }) : null,
      plot],
  });
}

const HEAT = ['rgba(255, 255, 255, .055)', '#3a2f6b', '#5b47ad', '#8067e0', '#b9a5fc']; // one hue, dim → bright
const isoDay = (dt) => `${dt.getFullYear()}-${String(dt.getMonth() + 1).padStart(2, '0')}-${String(dt.getDate()).padStart(2, '0')}`;

/** Every day of the period as one cell, a week per column. ←/→ move a week, ↑/↓ a day. */
function heatmap(d) {
  const months = (d.months || []).filter((m) => m && m.month);
  if (!months.length) return null;
  const first = monthDate(months[0].month);
  const lastM = monthDate(months[months.length - 1].month);
  const last = new Date(lastM.getFullYear(), lastM.getMonth() + 1, 0);
  const byDay = new Map((d.days || []).map((x) => [x.date, x]));
  // Steps follow the 95th percentile, so a single marathon day doesn't wash the rest of the year out.
  const sorted = [...byDay.values()].map((x) => x.watch_s).filter((x) => x > 0).sort((a, c) => a - c);
  const cap = sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95))] : 0;
  const level = (sec) => (sec > 0 && cap > 0 ? 1 + Math.min(3, Math.floor(Math.min(1, sec / cap) * 4)) : 0);
  const today = isoDay(new Date());

  const cells = [], labels = [];
  const dow = (dt) => (dt.getDay() + 6) % 7; // Monday first
  let col = 1, lastLabelCol = -9;
  for (let dt = new Date(first); dt <= last; dt.setDate(dt.getDate() + 1)) {
    const row = dow(dt) + 1;
    if (row === 1 && cells.length) col++;
    if (dt.getDate() === 1) {
      const at = row === 1 ? col : col + 1; // the first full week of the month
      if (at - lastLabelCol >= 3) { labels.push(h('span', { class: 'rc-heat-month mono', style: { gridColumn: String(at) } }, mShortF.format(dt))); lastLabelCol = at; }
    }
    const key = isoDay(dt), x = byDay.get(key), sec = x ? x.watch_s : 0;
    const cell = h('span', { class: 'rc-heat-cell' + (key > today ? ' is-future' : ''), style: { gridColumn: String(col), gridRow: String(row), background: HEAT[level(sec)] } });
    cell._d = { key, sec, plays: x ? x.plays : 0, col, row };
    cells.push(cell);
  }
  const cols = col;
  const at = (c, r) => cells.find((x) => x._d.col === c && x._d.row === r);
  let active = null;
  function setActive(cell) {
    if (active) active.classList.remove('is-active');
    active = cell || null;
    if (!active) { hideTip(); return; }
    active.classList.add('is-active');
    const x = active._d;
    showTip(active.getBoundingClientRect(), h('div', null,
      h('div', { class: 'tooltip-title' }, longDay(x.key)),
      h('div', { class: 'tooltip-row' }, h('span', { class: 'tooltip-key', style: { background: HEAT[Math.max(1, level(x.sec))] } }),
        x.sec > 0 ? [h('strong', null, duration(x.sec)), h('span', null, plural(x.plays, 'play', 'plays'))] : h('span', null, 'Nothing played'))));
  }
  const grid = h('div', { class: 'rc-heat-grid', style: { gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))` }, 'aria-hidden': 'true' }, cells);
  grid.addEventListener('pointerover', (e) => { if (e.target._d) setActive(e.target); });
  const activeDays = sorted.length;
  const wrap = h('div', { class: 'rc-heat', tabindex: 0, role: 'group',
    'aria-label': `Watch time for every day: ${plural(activeDays, 'day', 'days')} with plays. Use the arrow keys to read single days.` },
    h('div', { class: 'rc-heat-scroll' },
      h('div', { class: 'rc-heat-body', style: { minWidth: cols * 13 + 34 + 'px' } },
        h('div', { class: 'rc-heat-months', style: { gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))` }, 'aria-hidden': 'true' }, labels),
        h('div', { class: 'rc-heat-rows' },
          h('div', { class: 'rc-heat-dow mono', 'aria-hidden': 'true' }, ['Mon', '', 'Wed', '', 'Fri', '', ''].map((x) => h('span', null, x))),
          grid))),
    h('div', { class: 'rc-heat-legend mono', 'aria-hidden': 'true' }, h('span', null, 'Less'), HEAT.map((c) => h('i', { style: { background: c } })), h('span', null, 'More')),
    h('p', { class: 'sr-only', 'aria-live': 'polite' }));
  const live = wrap.lastChild;
  wrap.addEventListener('pointerleave', () => { if (document.activeElement !== wrap) setActive(null); });
  wrap.addEventListener('blur', () => setActive(null));
  wrap.addEventListener('focus', () => { if (!active) setActive(cells.find((x) => x._d.sec > 0) || cells[0]); });
  wrap.addEventListener('keydown', (e) => {
    const move = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[e.key];
    if (e.key === 'Escape') { setActive(null); return; }
    if (!move) return;
    e.preventDefault();
    const from = active ? active._d : cells[0]._d;
    const next = at(from.col + move[0], from.row + move[1]);
    if (!next) return;
    setActive(next);
    next.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    live.textContent = `${longDay(next._d.key)}: ${next._d.sec > 0 ? `${duration(next._d.sec)}, ${plural(next._d.plays, 'play', 'plays')}` : 'nothing played'}`;
  });
  return wrap;
}

function daysChapter(d, v) {
  const map = heatmap(d);
  const t = d.totals || {};
  if (!map || !t.active_days) return null;
  const r = d.records || {};
  const streak = r.longest_streak && r.longest_streak.days > 1 ? r.longest_streak : null;
  return chapter({
    id: 'days', icon: 'calendar', eyebrow: `${v.your} year in days`, mark: 'Days',
    title: [em(num(t.active_days)), ` ${t.active_days === 1 ? 'day' : 'days'} with something on`],
    lead: streak
      ? [`${v.your} longest streak ran `, b(`${num(streak.days)} days`), `, ${dayRange(streak.from, streak.to)}`,
        r.biggest_day ? tail(` — and the biggest day of all was ${longDay(r.biggest_day.date)}, with ${duration(r.biggest_day.watch_s)}.`) : '.']
      : r.biggest_day ? ['The biggest day was ', b(longDay(r.biggest_day.date)), tail(` — ${duration(r.biggest_day.watch_s)} in one go.`)] : null,
    body: map,
  });
}

function recordsChapter(d, v, isYear) {
  const r = d.records || {};
  const cards = [];
  if (r.biggest_day) cards.push(recordCard({ label: 'Biggest day', value: duration(r.biggest_day.watch_s), context: `${longDay(r.biggest_day.date)} · ${plural(r.biggest_day.plays, 'play', 'plays')}` }));
  if (r.biggest_binge) cards.push(recordCard({ label: 'Biggest binge', value: plural(r.biggest_binge.episodes, 'episode', 'episodes'),
    context: `of ${r.biggest_binge.series_name || 'one show'} on ${longDay(r.biggest_binge.date)}${r.biggest_binge.watch_s ? ` · ${duration(r.biggest_binge.watch_s)}` : ''}`,
    image: r.biggest_binge.image_item_id || null, name: r.biggest_binge.series_name }));
  if (r.longest_streak && r.longest_streak.days > 1) cards.push(recordCard({ label: 'Longest streak', value: `${num(r.longest_streak.days)} days in a row`, context: dayRange(r.longest_streak.from, r.longest_streak.to) }));
  if (r.longest_play) cards.push(recordCard({ label: 'Longest single play', value: duration(r.longest_play.duration_s),
    context: [r.longest_play.name, r.longest_play.date ? longDay(r.longest_play.date) : null].filter(Boolean).join(' · '), image: r.longest_play.image_item_id || null, name: r.longest_play.name }));
  if (r.most_rewatched) cards.push(recordCard({ label: 'Most rewatched', value: `${num(r.most_rewatched.plays)} times`, context: r.most_rewatched.name, image: r.most_rewatched.image_item_id || null, name: r.most_rewatched.name }));
  if (r.first_play) cards.push(recordCard({ label: isYear ? 'First play of the year' : 'First play of the period', value: r.first_play.name || 'Unknown title',
    context: r.first_play.at ? `${longDate(new Date(r.first_play.at * 1000))} at ${hmF.format(new Date(r.first_play.at * 1000))}` : null,
    image: r.first_play.image_item_id || null, name: r.first_play.name }));
  if (r.oldest_title) cards.push(recordCard({ label: 'Oldest title', value: r.oldest_title.year ? `From ${r.oldest_title.year}` : r.oldest_title.name, context: r.oldest_title.name, image: r.oldest_title.image_item_id || null, name: r.oldest_title.name }));
  if (!cards.length) return null;
  return chapter({ id: 'records', icon: 'trophy', eyebrow: 'Records', mark: 'Records',
    title: v.you ? ['The days you’ll ', em('remember')] : ['The days that ', em('stood out')], body: h('div', { class: 'rc-records' }, cards) });
}

function discoveryChapter(d, v) {
  const x = d.discovery;
  if (!x) return null;
  const once = (x.one_and_done || []).filter(Boolean);
  if (!x.new_series && !x.finished_movies && !x.finished_episodes && !once.length) return null;
  return chapter({
    id: 'discovery', icon: 'compass', eyebrow: 'Discovery', mark: 'New',
    title: x.new_series ? [em(plural(x.new_series, 'show', 'shows')), v.you ? ' you’d never seen before' : ` ${v.who} had never seen before`] : ['Seen to the ', em('end')],
    stats: [
      { value: countUp(x.new_series || 0), label: 'Shows started' },
      { value: countUp(x.finished_movies || 0), label: 'Movies finished' },
      { value: countUp(x.finished_episodes || 0), label: 'Episodes finished' },
    ],
    body: once.length ? h('div', { class: 'rc-once' },
      h('h3', { class: 'rc-subhead' }, 'One and done'),
      h('p', { class: 'rc-note' }, 'Shows that got exactly one episode and never a second.'),
      h('ul', { class: 'rc-posterrow' }, once.map((s) => h('li', null, poster(s.image_item_id, s.name, { w: 300, cls: 'rc-posterrow-poster' }),
        s.id ? h('a', { class: 'rc-link rc-posterrow-name', href: `/items/${s.id}` }, s.name || 'Unknown show') : h('span', { class: 'rc-posterrow-name' }, s.name || 'Unknown show'))))) : null,
  });
}

function clientsChapter(d, v) {
  const rows = (d.clients || []).filter((c) => c && c.name);
  if (!rows.length) return null;
  const total = (d.totals && d.totals.plays) || rows.reduce((a, c) => a + (c.plays || 0), 0) || 1;
  return chapter({
    id: 'clients', icon: 'monitor', eyebrow: v.you ? 'How you watched' : `How ${v.who} watched`, mark: 'Screens',
    title: ['Mostly on ', em(rows[0].name)],
    body: h('ul', { class: 'rc-chips' }, rows.map((c) => h('li', { class: 'rc-chip' }, h('span', { class: 'rc-chip-name' }, c.name), h('span', { class: 'rc-chip-val mono' }, pct((c.plays || 0) / total), ' of plays')))),
  });
}

/** The end, the way films end: the year's credits. */
function outroChapter(d, v, periodLabel) {
  const isYear = /^\d{4}$/.test(periodLabel);
  const names = (rows, n) => (rows || []).slice(0, n).map((x) => x && x.name).filter(Boolean);
  const p = d.people || {};
  const credits = [
    ['Starring', names(d.top_series, 3)],
    ['Feature presentation', names(d.top_movies, 1)],
    ['With', names(p.actors, 2)],
    ['Directed by', names(p.directors, 1)],
    ['Soundtrack', names(d.top_tracks, 1)],
    ['Genre', names(d.top_genres, 1)],
    ['Screened on', names(d.clients, 1)],
    ['Running time', d.totals && d.totals.watch_s ? [duration(d.totals.watch_s)] : []],
  ].filter(([, list]) => list.length);
  const rows = credits.map(([role, list]) => h('div', { class: 'rc-credit' }, h('dt', null, role), h('dd', null, list.map((n) => h('span', null, n)))));
  rows.forEach((el, i) => el.style.setProperty('--i', String(i)));
  return h('section', { class: 'rc-credits rc-reveal', id: 'rc-outro', 'aria-label': 'Credits' },
    h('p', { class: 'rc-eyebrow' }, icon('film', 15), h('span', null, 'Roll credits')),
    h('h2', { class: 'rc-title' }, isYear ? ['That was ', em(periodLabel)] : ['That was the last ', em('12 months')]),
    h('dl', { class: 'rc-credits-list' }, rows),
    h('p', { class: 'rc-credits-line' }, v.you ? 'Everything you watch from here on is already counting towards the next one.' : `Everything ${v.who} watches from here on is already counting towards the next one.`),
    h('div', { class: 'rc-outro-links' }, h('a', { class: 'btn', href: '/activity' }, icon('activity', 14), 'See all activity'),
      d.scope && d.scope.user_id ? h('a', { class: 'btn btn-ghost', href: `/users/${d.scope.user_id}` }, icon('user', 14), v.you ? 'Your profile' : `${v.your} profile`) : null));
}

// ---------------------------------------------------------------- page
function buildStory(d, me, periodLabel, picker) {
  const v = voiceFor(d.scope, me);
  const parts = [
    heroChapter(d, v, periodLabel, picker),
    numbersChapter(d, v, periodLabel),
    topChapter('shows', d.top_series, {
      icon: 'tv', eyebrow: 'Top shows', mark: 'Shows',
      title: (t) => [em(t.name), v.you ? ' owned your year' : ` owned ${v.your} year`],
      lead: (t) => [b(`${hoursText(t.watch_s)} hours`), ' together', t.episodes ? [', over ', b(plural(t.episodes, 'episode', 'episodes'))] : null, tail(' — more than any other show.')],
      facts: (t) => [['Watch time', duration(t.watch_s)], t.episodes ? ['Episodes', num(t.episodes)] : null, ['Plays', num(t.plays)]],
      extra: (t) => (t.episodes ? plural(t.episodes, 'episode', 'episodes') : null),
    }),
    topChapter('movies', d.top_movies, {
      icon: 'film', eyebrow: 'Top movies', mark: 'Movies',
      title: (t) => [`${v.your} movie of the year: `, em(t.name)],
      lead: (t) => [b(duration(t.watch_s)), ' in total', t.plays > 1 ? tail(` — ${v.whoLow} came back to it ${num(t.plays)} times.`) : '.'],
      facts: (t) => [['Watch time', duration(t.watch_s)], ['Plays', num(t.plays)]],
      extra: (t) => plural(t.plays, 'play', 'plays'),
    }),
    d.top_tracks && d.top_tracks.length ? chapter({ id: 'tracks', icon: 'volume', eyebrow: 'Top tracks', mark: 'Repeat', title: ['On ', em('repeat')], body: rankedList(d.top_tracks, 1, (t) => plural(t.plays, 'play', 'plays')) }) : null,
    genresChapter(d, v),
    peopleChapter(d, v),
    personaChapter(d, v),
    rhythmChapter(d, v),
    daysChapter(d, v),
    recordsChapter(d, v, /^\d{4}$/.test(periodLabel)),
    discoveryChapter(d, v),
    clientsChapter(d, v),
    outroChapter(d, v, periodLabel),
  ];
  return parts.filter(Boolean);
}

export default function recapPage(ctx) {
  ctx.title('Recap');
  const me = state.user;
  const qYear = ctx.query.get('year');
  let year = /^\d{4}$/.test(qYear || '') || qYear === 'last12' ? qYear : '';
  // Administrators can open one other person's recap. There is no "everyone" recap.
  let userId = isAdmin() ? ctx.query.get('user') || '' : '';
  let reveal = revealer();
  ctx.onCleanup(() => { reveal.stop(); hideTip(); });

  const controls = isAdmin() ? h('div', { class: 'filters rc-controls' },
    combobox({ value: userId, allLabel: 'My recap', placeholder: 'My recap', label: 'Whose recap',
      load: () => userList(ctx.signal).then((us) => us.filter((u) => !me || u.id !== me.id).map((u) => ({ value: u.id, label: u.name }))),
      onChange: (val) => { userId = val; year = ''; sync(); dv.load(); } })) : null;
  const view = h('div', { class: 'rc-story' });

  const periodLabel = (y) => (y === 'last12' ? 'Last 12 months' : String(y));
  const sync = () => replaceQuery({ year, user: userId });

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [h('div', { class: 'rc-hero rc-sk' }, h('div', { class: 'rc-hero-text' }, sk.line('120px', 12), sk.line('min(80%, 520px)', 64), sk.line('min(90%, 360px)', 16), sk.block(120))),
      h('div', { class: 'rc-chapter is-in' }, sk.line('120px', 12), sk.line('min(80%, 380px)', 30), sk.block(220)),
      h('div', { class: 'rc-chapter is-in' }, sk.line('120px', 12), sk.line('min(80%, 380px)', 30), sk.block(220))],
    fetch: () => api.get('/recap', { year, user_id: userId }, { signal: ctx.signal }),
    render: (d) => {
      if (d.year != null) year = String(d.year);
      const picker = yearTabs(d, String(year || ''), (val) => { year = val; sync(); dv.load(); });
      const label = periodLabel(year || 'this period');
      ctx.title(`Recap ${label}`);
      reveal.stop();
      reveal = revealer();
      view.classList.toggle('rc-anim', reveal.animated); // content is only ever hidden when something will reveal it
      if (d.empty || !d.totals || !d.totals.plays) {
        const v = voiceFor(d.scope, me);
        return [h('div', { class: 'rc-empty-picker' }, picker),
          emptyState(`Nothing was played in ${year === 'last12' ? 'the last 12 months' : label}.`, `${v.who} didn’t play anything in this period. Pick another one above.`)];
      }
      const story = buildStory(d, me, label, picker);
      // Observe after mount so the first screen reveals immediately.
      requestAnimationFrame(() => story.forEach((el) => { if (el.classList.contains('rc-reveal')) reveal.watch(el); }));
      return story;
    },
  });

  ctx.root.append(...[h('h1', { class: 'sr-only' }, 'Recap'), controls ? h('div', { class: 'rc-top-row' }, controls) : null, view].filter(Boolean));
  ctx.root.classList.add('rc-page');
  ctx.onCleanup(() => ctx.root.classList.remove('rc-page'));
  dv.load();
}

/** Dashboard banner, December and January only. */
export function recapBanner() {
  const now = new Date();
  const m = now.getMonth();
  if (m !== 11 && m !== 0) return null;
  const y = m === 11 ? now.getFullYear() : now.getFullYear() - 1;
  return h('a', { class: 'recap-banner', href: `/recap?year=${y}` }, icon('recap', 15),
    h('span', null, `Your ${y} recap is ready`), icon('chevronRight', 14, 'recap-banner-go'));
}
