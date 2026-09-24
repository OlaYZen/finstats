// Patch notes: every release, newest first, straight from CHANGELOG.md.

import { h, icon } from '../dom.js';
import { api } from '../api.js';
import { pageHeader, dataView, emptyState, sk } from '../components.js';
import { markVersionSeen } from '../state.js';

const KINDS = {
  Added: { icon: 'plus', cls: 'cl-added' },
  Changed: { icon: 'sliders', cls: 'cl-changed' },
  Fixed: { icon: 'check', cls: 'cl-fixed' },
  Removed: { icon: 'x', cls: 'cl-removed' },
};

const calm = () => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;

/** Notes may use **bold** and `code`. Built as DOM nodes; nothing is ever parsed as HTML. */
function inline(text) {
  const out = [];
  const re = /\*\*([^*]+)\*\*|`([^`]+)`/g;
  let last = 0;
  let m;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    out.push(m[1] != null ? h('strong', null, m[1]) : h('code', { class: 'mono cl-code' }, m[2]));
    last = re.lastIndex;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

function longDate(iso) {
  const d = /^\d{4}-\d{2}-\d{2}$/.test(iso || '') ? new Date(iso + 'T12:00:00') : null;
  return d && !isNaN(d) ? d.toLocaleDateString(undefined, { year: 'numeric', month: 'long', day: 'numeric' }) : iso || '';
}

function release(r, current) {
  const running = r.version === current;
  return h('article', { class: ['cl-release', running && 'is-current'], 'aria-labelledby': `cl-${r.version}`, dataset: { version: r.version } },
    h('header', { class: 'cl-head' },
      h('h2', { class: 'cl-version mono', id: `cl-${r.version}` }, 'v' + r.version),
      running ? h('span', { class: 'chip cl-running' }, icon('check', 12), 'Running now') : null,
      r.date ? h('time', { class: 'cl-date', dateTime: r.date }, longDate(r.date)) : null),
    r.summary ? h('p', { class: 'cl-summary' }, inline(r.summary)) : null,
    (r.groups || []).filter((g) => g.items && g.items.length).map((g) => {
      const k = KINDS[g.kind] || KINDS.Changed;
      return h('section', { class: 'cl-group' },
        h('h3', { class: ['cl-kind', k.cls] }, icon(k.icon, 12), g.kind),
        h('ul', { class: 'cl-items' }, g.items.map((it) => h('li', null, inline(it)))));
    }));
}

/** Releases of one minor series (0.7.0 … 0.7.3) fold into one group, newest series first. */
function series(releases) {
  const groups = [];
  for (const r of releases) {
    const key = String(r.version).split('.').slice(0, 2).join('.');
    const last = groups[groups.length - 1];
    if (last && last.key === key) last.releases.push(r); else groups.push({ key, releases: [r] });
  }
  return groups;
}

function group(g, current, anchors) {
  const newest = g.releases[0];
  const oldest = g.releases[g.releases.length - 1];
  const running = g.releases.some((r) => r.version === current);
  const n = g.releases.length;
  const dates = newest.date && oldest.date && newest.date !== oldest.date ? `${longDate(oldest.date)} – ${longDate(newest.date)}` : longDate(newest.date);
  // The x.y.0 release says what the series was about; fall back to the newest summary.
  // A title line carries no full stop, so it is taken whole; anything that is a sentence
  // gives only its first, so a headline is never an introduction cut off by an ellipsis.
  const about = (oldest.summary || newest.summary || '').trim();
  const headline = (about.match(/^.+?[.!?](?=\s|$)/) || [about])[0];
  const articles = g.releases.map((r) => release(r, current));
  // Nothing folds: every series stands open and the list of versions is how you get about.
  // A page that hides most of itself behind a click is worse than a long one you can jump around.
  const section = h('section', { class: ['cl-series', running && 'is-current'], dataset: { key: g.key } },
    h('div', { class: 'cl-series-head' },
      h('span', { class: 'cl-series-name mono' }, `v${g.key}`),
      h('span', { class: 'cl-series-range mono' }, n > 1 ? `${oldest.version} – ${newest.version}` : newest.version),
      headline ? h('span', { class: 'cl-series-about' }, inline(headline)) : null,
      h('span', { class: 'cl-series-meta' }, `${n} ${n === 1 ? 'release' : 'releases'}`, dates ? ` · ${dates}` : '')),
    h('div', { class: 'cl-series-body' }, articles));
  // The newest release of a series sits directly under its own heading, so going to it means
  // going to the series; anything older is scrolled to on its own.
  g.releases.forEach((r, i) => anchors.set(r.version, i === 0 ? section : articles[i]));
  return { key: g.key, section, articles };
}

/** The list beside the notes: every series, every version, one click to any of them. */
function contents(groups, current, go) {
  const rows = new Map();
  const heads = new Map();
  const node = h('nav', { class: 'cl-toc', 'aria-label': 'Versions' },
    h('p', { class: 'cl-toc-title' }, 'Versions'),
    h('ul', { class: 'cl-toc-list' }, groups.map((g) => {
      const head = h('button', { type: 'button', class: 'cl-toc-head', onClick: () => go(g.key, g.releases[0].version) },
        h('span', { class: 'cl-toc-key mono' }, `v${g.key}`),
        h('span', { class: 'cl-toc-n' }, String(g.releases.length)));
      const li = h('li', { class: 'cl-toc-series' }, head,
        h('ul', { class: 'cl-toc-vers' }, g.releases.map((r) => {
          const b = h('button', { type: 'button', class: ['cl-toc-ver', 'mono', r.version === current && 'is-running'], onClick: () => go(g.key, r.version) }, r.version);
          rows.set(r.version, b);
          return h('li', null, b);
        })));
      heads.set(g.key, li);
      return li;
    })));
  return { node, rows, heads };
}

// Shared with the prefetcher, so a prefetched view has exactly the address the page asks for.
const loadChangelog = (signal) => api.get('/changelog', null, { signal });
export const prefetchChangelog = ({ signal }) => [() => loadChangelog(signal)];

export default function changelogPage(ctx) {
  ctx.title('Patch notes');
  const view = h('div', { class: 'cl-page' });
  ctx.root.append(pageHeader('Patch notes', 'What changed in finstats, newest first'), view);
  let spy = null;
  ctx.onCleanup(() => spy?.disconnect());
  dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.block(180), sk.block(56), sk.block(56)],
    fetch: () => loadChangelog(ctx.signal),
    render: (d) => {
      markVersionSeen(d.current);
      const releases = d.releases || [];
      if (!releases.length) return emptyState('No patch notes yet.', 'This build was made without a changelog.');

      const anchors = new Map();
      const groups = series(releases);
      const sections = groups.map((g) => group(g, d.current, anchors));
      const list = h('div', { class: 'cl-list' }, sections.map((f) => f.section));

      const go = (key, version) => anchors.get(version)?.scrollIntoView({ behavior: calm() ? 'auto' : 'smooth', block: 'start' });
      const toc = contents(groups, d.current, go);

      // Which version is being read, marked in the list, together with the series it belongs to.
      const order = releases.map((r) => r.version);
      const of = new Map(sections.flatMap((f) => f.articles.map((a) => [a.dataset.version, f.key])));
      const onScreen = new Set();
      let at = null;
      spy?.disconnect();
      spy = new IntersectionObserver((entries) => {
        for (const e of entries) {
          if (e.isIntersecting) onScreen.add(e.target.dataset.version); else onScreen.delete(e.target.dataset.version);
        }
        const top = order.find((v) => onScreen.has(v)) || null;
        if (top === at) return;
        if (at) toc.rows.get(at)?.classList.remove('is-at');
        at = top;
        for (const [k, li] of toc.heads) li.classList.toggle('is-at', k === of.get(at));
        const row = at && toc.rows.get(at);
        if (!row) return;
        row.classList.add('is-at');
        // Keep that row in view without ever moving the page: only the list itself scrolls.
        const box = toc.node;
        if (box.scrollHeight <= box.clientHeight) return;
        const r = row.getBoundingClientRect();
        const b = box.getBoundingClientRect();
        if (r.top < b.top + 4) box.scrollTop += r.top - b.top - 4;
        else if (r.bottom > b.bottom - 4) box.scrollTop += r.bottom - b.bottom + 4;
      }, { rootMargin: '-12% 0px -72% 0px' });
      for (const f of sections) for (const a of f.articles) spy.observe(a);

      return h('div', { class: 'cl-wrap' }, toc.node, list);
    },
  }).load();
}
