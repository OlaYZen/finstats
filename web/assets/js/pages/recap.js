// Recap — the year in review. The one loud page in an otherwise quiet app:
// editorial numerals, poster art as the material, one idea per chapter.

import { h, icon, mount, num, duration, durationExact, pct, parseDay } from '../dom.js';
import { api, imgItem } from '../api.js';
import { state } from '../state.js';
import { replaceQuery } from '../router.js';
import { dataView, segmented, poster, emptyState, sk } from '../components.js';
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
/** A recap is always the signed-in person's own, so the copy speaks to them directly. */
const YOU = { who: 'You', whoLow: 'you', your: 'Your', yourLow: 'your' };

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
function chapter({ id, eyebrow, title, lead, body, cls = '' }) {
  const kids = [
    eyebrow ? h('p', { class: 'rc-eyebrow mono' }, eyebrow) : null,
    title ? h('h2', { class: 'rc-title' }, title) : null,
    lead ? h('p', { class: 'rc-lead' }, lead) : null,
    ...(Array.isArray(body) ? body : [body]),
  ].filter(Boolean);
  kids.forEach((k, i) => k.style.setProperty('--i', String(i)));
  return h('section', { class: 'rc-chapter rc-reveal ' + cls, id: id ? 'rc-' + id : null, 'aria-label': eyebrow || title }, kids);
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
function heroChapter(d, v, periodLabel) {
  const t = d.totals || {};
  const tiles = [...(d.top_series || []), ...(d.top_movies || [])].filter((x) => x && x.image_item_id).slice(0, 10);
  const strip = tiles.length ? h('div', { class: 'rc-hero-strip', 'aria-hidden': 'true' }, tiles.map((x) =>
    h('img', { src: imgItem(x.image_item_id, 300), alt: '', decoding: 'async', onError: (e) => e.target.remove() }))) : null;
  const isYear = /^\d{4}$/.test(periodLabel);
  const eyebrow = `${v.your} ${isYear ? periodLabel : periodLabel.toLowerCase()}`;
  const hrs = hoursOf(t.watch_s);
  const days = hrs / 24;
  const daysText = days >= 10 ? num(days) : (Math.round(days * 10) / 10).toLocaleString();
  const sentence = [
    hrs >= 24 ? `That’s ${daysText} ${daysText === '1' ? 'day' : 'days'} without a break. ` : '',
    `${v.who} pressed play ${plural(t.plays, 'time', 'times')} on ${plural(t.distinct_items, 'different title', 'different titles')}`,
    t.active_days ? `, across ${plural(t.active_days, 'day', 'days')}.` : '.',
  ].join('');
  const r = d.rank;
  return h('header', { class: 'rc-hero rc-reveal' }, strip, h('div', { class: 'rc-hero-glow', 'aria-hidden': 'true' }),
    h('div', { class: 'rc-hero-text' },
      h('p', { class: 'rc-eyebrow mono' }, eyebrow),
      h('p', { class: 'rc-hero-num' }, countUp(hrs, (x) => (hrs >= 10 ? num(x) : (Math.round(x * 10) / 10).toLocaleString())), h('span', { class: 'rc-hero-unit' }, hrs === 1 ? 'hour watched' : 'hours watched')),
      h('p', { class: 'rc-hero-line' }, sentence),
      r && r.position ? h('p', { class: 'rc-hero-rank mono' }, `#${num(r.position)} of ${plural(r.of, 'viewer', 'viewers')}${r.share != null ? ` · ${pct(r.share)} of all watching` : ''}`) : null));
}

function topChapter(id, rows, { eyebrow, title, lead, facts, extra }) {
  if (!rows || !rows.length) return null;
  const [first, ...rest] = rows;
  return chapter({ id, eyebrow, title, lead: lead(first), body: h('div', { class: 'rc-top' }, feature(first, facts(first)), rankedList(rest, 2, extra)) });
}

function personaChapter(d, v) {
  const p = d.persona;
  const hours = Array.isArray(d.hours) && d.hours.length === 24 ? d.hours : null;
  const weekdays = Array.isArray(d.weekdays) && d.weekdays.length === 7 ? d.weekdays : null;
  const hasHours = !!hours && hours.some((x) => x > 0), hasDays = !!weekdays && weekdays.some((x) => x > 0);
  if (!p && !hasHours && !hasDays) return null;
  return chapter({
    id: 'persona', eyebrow: 'Viewing personality', cls: 'rc-persona',
    title: p ? h('span', { class: 'rc-persona-title' }, p.title || 'Creature of habit') : 'When you watch',
    lead: p && p.line ? p.line : null,
    body: !hasHours && !hasDays ? null : h('div', { class: 'rc-persona-charts' },
      hasHours ? h('figure', { class: 'rc-figure' }, h('figcaption', null, 'Around the clock', h('span', null, 'Watch time by hour of day')),
        barStrip({ rows: hours.map((x, i) => ({ label: String(i).padStart(2, '0'), title: `${hour2(i)} – ${hour2((i + 1) % 24)}`, value: x })), format: duration, ariaLabel: 'Watch time by hour of day', head: ['Hour', 'Watch time'], labelEvery: 3, cls: 'rc-bars-24' })) : null,
      hasDays ? h('figure', { class: 'rc-figure' }, h('figcaption', null, 'Through the week', h('span', null, 'Watch time by weekday')),
        barStrip({ rows: weekdays.map((x, i) => ({ label: WEEKDAYS[i].slice(0, 3), title: WEEKDAYS[i], value: x })), format: duration, ariaLabel: 'Watch time by weekday', head: ['Weekday', 'Watch time'], cls: 'rc-bars-7' })) : null),
  });
}

function monthsChapter(d, v) {
  const months = (d.months || []).filter((m) => m && m.month);
  if (!months.length || !months.some((m) => m.watch_s > 0)) return null;
  const spansYears = new Set(months.map((m) => String(m.month).slice(0, 4))).size > 1;
  const best = months.reduce((a, b) => (b.watch_s > a.watch_s ? b : a));
  const bestDate = monthDate(best.month);
  const rows = months.map((m) => {
    const dt = monthDate(m.month);
    const name = m.top && m.top.name;
    return {
      label: mShortF.format(dt), title: `${mF.format(dt)}${spansYears ? ' ' + dt.getFullYear() : ''}`, value: m.watch_s,
      note: name ? `mostly ${name}` : null,
      foot: h('span', { class: 'rc-col-foot', title: name || null }, m.top ? [poster(m.top.image_item_id, name, { w: 120, cls: 'poster-xs' }), h('span', { class: 'rc-col-foot-name' }, name)] : null),
    };
  });
  return chapter({
    id: 'months', eyebrow: 'Month by month',
    title: `${mF.format(bestDate)} was the big one.`,
    lead: `${duration(best.watch_s)} in a single month${best.top && best.top.name ? `, most of it with ${best.top.name}` : ''}.`,
    body: [
      barStrip({ rows, format: duration, ariaLabel: 'Watch time per month', head: ['Month', 'Watch time'], cls: 'rc-bars-months' }),
      h('table', { class: 'sr-only' }, h('caption', null, 'Most watched title per month'),
        h('tbody', null, rows.map((r, i) => h('tr', null, h('td', null, r.title), h('td', null, (months[i].top && months[i].top.name) || 'Nothing'))))),
    ],
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
  return chapter({ id: 'records', eyebrow: 'Records', title: 'The days you’ll remember', body: h('div', { class: 'rc-records' }, cards) });
}

function discoveryChapter(d, v) {
  const x = d.discovery;
  if (!x) return null;
  const once = (x.one_and_done || []).filter(Boolean);
  if (!x.new_series && !x.finished_movies && !x.finished_episodes && !once.length) return null;
  const started = 'New shows you started';
  return chapter({
    id: 'discovery', eyebrow: 'Discovery', title: started,
    body: [
      h('div', { class: 'rc-bigrow' },
        h('div', { class: 'rc-big' }, h('span', { class: 'rc-big-num' }, countUp(x.new_series || 0)), h('span', { class: 'rc-big-label' }, 'shows played for the first time')),
        h('div', { class: 'rc-big rc-big-quiet' }, h('span', { class: 'rc-big-num' }, countUp(x.finished_movies || 0)), h('span', { class: 'rc-big-label' }, 'movies watched to the end')),
        h('div', { class: 'rc-big rc-big-quiet' }, h('span', { class: 'rc-big-num' }, countUp(x.finished_episodes || 0)), h('span', { class: 'rc-big-label' }, 'episodes finished'))),
      once.length ? h('div', { class: 'rc-once' },
        h('h3', { class: 'rc-subhead' }, 'One and done'),
        h('p', { class: 'rc-note' }, 'Shows that got exactly one episode and never a second.'),
        h('ul', { class: 'rc-posterrow' }, once.map((s) => h('li', null, poster(s.image_item_id, s.name, { w: 300, cls: 'rc-posterrow-poster' }),
          s.id ? h('a', { class: 'rc-link rc-posterrow-name', href: `/items/${s.id}` }, s.name || 'Unknown show') : h('span', { class: 'rc-posterrow-name' }, s.name || 'Unknown show'))))) : null,
    ],
  });
}

function clientsChapter(d, v) {
  const rows = (d.clients || []).filter((c) => c && c.name);
  if (!rows.length) return null;
  const total = (d.totals && d.totals.plays) || rows.reduce((a, c) => a + (c.plays || 0), 0) || 1;
  return chapter({
    id: 'clients', eyebrow: 'How you watched', title: `Mostly on ${rows[0].name}.`,
    body: h('ul', { class: 'rc-chips' }, rows.map((c) => h('li', { class: 'rc-chip' }, h('span', { class: 'rc-chip-name' }, c.name), h('span', { class: 'rc-chip-val mono' }, pct((c.plays || 0) / total), ' of plays')))),
  });
}

// ---------------------------------------------------------------- page
function buildStory(d, me, periodLabel) {
  const v = YOU;
  const parts = [
    heroChapter(d, v, periodLabel),
    topChapter('shows', d.top_series, {
      eyebrow: 'Top shows', title: `${v.your} most watched show`,
      lead: (t) => `${hoursText(t.watch_s)} hours with ${t.name}${t.episodes ? `, over ${plural(t.episodes, 'episode', 'episodes')}` : ''}.`,
      facts: (t) => [['Watch time', duration(t.watch_s)], t.episodes ? ['Episodes', num(t.episodes)] : null, ['Plays', num(t.plays)]],
      extra: (t) => (t.episodes ? plural(t.episodes, 'episode', 'episodes') : null),
    }),
    topChapter('movies', d.top_movies, {
      eyebrow: 'Top movies', title: `${v.your} top movie`,
      lead: (t) => `${t.name}${t.plays > 1 ? ` — played ${num(t.plays)} times` : ''}, ${duration(t.watch_s)} in total.`,
      facts: (t) => [['Watch time', duration(t.watch_s)], ['Plays', num(t.plays)]],
      extra: (t) => plural(t.plays, 'play', 'plays'),
    }),
    d.top_tracks && d.top_tracks.length ? chapter({ id: 'tracks', eyebrow: 'Top tracks', title: 'On repeat', body: rankedList(d.top_tracks, 1, (t) => plural(t.plays, 'play', 'plays')) }) : null,
    d.top_genres && d.top_genres.length ? chapter({
      id: 'genres', eyebrow: 'Genres', title: `${d.top_genres[0].name} led the way.`,
      lead: `${duration(d.top_genres[0].watch_s)} of ${d.top_genres[0].name}${d.top_genres[1] ? `, with ${d.top_genres[1].name} next` : ''}. Shows count towards their own genres, episode by episode.`,
      body: rankBars(d.top_genres, (g) => g.watch_s || 0, (g) => duration(g.watch_s)),
    }) : null,
    personaChapter(d, v),
    monthsChapter(d, v),
    recordsChapter(d, v, /^\d{4}$/.test(periodLabel)),
    discoveryChapter(d, v),
    clientsChapter(d, v),
    chapter({
      id: 'outro', cls: 'rc-outro', title: `That was ${periodLabel.toLowerCase().startsWith('last') ? 'the ' + periodLabel.toLowerCase() : periodLabel}.`,
      lead: 'Everything you watch from here on is already counting towards the next one.',
      body: [
        h('div', { class: 'rc-outro-links' }, h('a', { class: 'btn', href: '/activity' }, icon('activity', 14), 'See all activity'),
          me && me.id ? h('a', { class: 'btn btn-ghost', href: `/users/${me.id}` }, icon('user', 14), 'Your stats') : null),
      ],
    }),
  ];
  return parts.filter(Boolean);
}

export default function recapPage(ctx) {
  ctx.title('Recap');
  const me = state.user;
  const qYear = ctx.query.get('year');
  let year = /^\d{4}$/.test(qYear || '') || qYear === 'last12' ? qYear : '';
  let reveal = revealer();
  ctx.onCleanup(() => { reveal.stop(); hideTip(); });

  const yearSlot = h('div', { class: 'rc-yearslot' });
  const controls = h('div', { class: 'filters rc-controls' }, yearSlot);
  const view = h('div', { class: 'rc-story' });

  const periodLabel = (y) => (y === 'last12' ? 'Last 12 months' : String(y));
  function paintYears(d) {
    const current = String(d.year != null ? d.year : year || '');
    let years = (d.years || []).map(String);
    const many = years.length + 1 > 5;
    if (many) years = years.slice(0, 4);
    if (/^\d{4}$/.test(current) && !years.includes(current)) years = [current, ...years].sort().reverse().slice(0, many ? 4 : 5);
    const options = [...years.map((y) => ({ value: y, label: y })), { value: 'last12', label: 'Last 12 months' }];
    mount(yearSlot, segmented({ label: 'Period', value: current, options, onChange: (val) => { year = val; sync(); dv.load(); } }));
  }
  const sync = () => replaceQuery({ year });

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [h('div', { class: 'rc-hero rc-sk' }, h('div', { class: 'rc-hero-text' }, sk.line('160px', 12), sk.line('min(70%, 420px)', 96), sk.line('min(90%, 520px)', 16))),
      h('div', { class: 'rc-chapter is-in' }, sk.line('120px', 12), sk.line('min(80%, 380px)', 30), sk.block(220)),
      h('div', { class: 'rc-chapter is-in' }, sk.line('120px', 12), sk.line('min(80%, 380px)', 30), sk.block(220))],
    fetch: () => api.get('/recap', { year }, { signal: ctx.signal }),
    render: (d) => {
      if (d.year != null) year = String(d.year);
      paintYears(d);
      const label = periodLabel(year || 'this period');
      ctx.title(`Recap ${label}`);
      reveal.stop();
      reveal = revealer();
      view.classList.toggle('rc-anim', reveal.animated); // content is only ever hidden when something will reveal it
      if (d.empty || !d.totals || !d.totals.plays) {
        return emptyState(`Nothing was played in ${year === 'last12' ? 'the last 12 months' : label}.`, 'You didn’t play anything in this period. Pick another one above.');
      }
      const story = buildStory(d, me, label);
      // Observe after mount so the first screen reveals immediately.
      requestAnimationFrame(() => story.forEach((el) => { if (el.classList.contains('rc-reveal')) reveal.watch(el); }));
      return story;
    },
  });

  ctx.root.append(
    h('div', { class: 'rc-top-row' }, h('h1', { class: 'sr-only' }, 'Recap'), controls),
    view);
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
