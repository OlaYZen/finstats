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
  return h('article', { class: ['cl-release', running && 'is-current'], 'aria-labelledby': `cl-${r.version}` },
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

export default function changelogPage(ctx) {
  ctx.title('Patch notes');
  const view = h('div', { class: 'cl-list' });
  ctx.root.append(pageHeader('Patch notes', 'What changed in finstats, newest first'), view);
  dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.block(180), sk.block(140), sk.block(140)],
    fetch: () => api.get('/changelog', null, { signal: ctx.signal }),
    render: (d) => {
      markVersionSeen(d.current);
      const releases = d.releases || [];
      if (!releases.length) return emptyState('No patch notes yet.', 'This build was made without a changelog.');
      return releases.map((r) => release(r, d.current));
    },
  }).load();
}
