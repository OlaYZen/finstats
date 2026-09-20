// /users/:id/timeline — one person's watching as a trail, newest first. Wide screens lay it out as a
// snake (left to right, then back), a phone gets one straight line.
import { h, icon, duration, durationExact, dateTime } from '../dom.js';
import { api } from '../api.js';
import { replaceQuery } from '../router.js';
import { dataView, sk, avatar, poster, emptyState, errorState } from '../components.js';
import { userTabs } from './users.js';

const COL_MIN = 330;   // a stop is never narrower than this in the snake
const LINE_BELOW = 620; // below this width the trail is a straight line
const MAX_COLS = 4;

const monthf = new Intl.DateTimeFormat(undefined, { month: 'short', year: 'numeric' });
const dayf = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short' });
const dayfy = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
const timef = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', hour12: false });
const TYPE_CLASS = { Movie: 'is-movie', Episode: 'is-episode', Audio: 'is-audio' };

export function timelinePage(ctx) {
  const id = ctx.params.id;
  ctx.title('Timeline');
  // null = every library; otherwise exactly these (never none: switching the last one off shows all again).
  let picked = new Set(String(ctx.query.libraries || '').split(',').filter((l) => /^[0-9a-f]{32}$/.test(l)));
  if (!picked.size) picked = null;
  const headerSlot = h('div');
  const filterSlot = h('div');
  const view = h('div');
  const params = (extra = {}) => ({ ...(picked ? { libraries: [...picked].join(',') } : {}), ...extra });

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => h('div', { class: 'trail-sk' }, [0, 1, 2, 3, 4, 5].map(() => h('span', { class: 'sk trail-sk-stop' }))),
    fetch: () => api.get(`/users/${id}/timeline`, params(), { signal: ctx.signal }),
    render: (d) => {
      paintHeader(d.user);
      paintFilter(d.libraries || []);
      if (!d.stops.length) return emptyState('Nothing here yet', picked ? 'No plays in the libraries you picked.' : 'Plays show up here as soon as something has been watched.');
      return trailView(d);
    },
  });

  function paintHeader(u) {
    ctx.title(`${u.name} · Timeline`);
    headerSlot.replaceChildren(h('header', { class: 'page-header entity-header' },
      avatar(u.id, u.name, { size: 56, hasImage: u.has_image }),
      h('div', null, h('h1', { class: 'page-title' }, u.name), h('p', { class: 'page-sub' }, 'Everything watched, newest first'))));
  }

  function paintFilter(libraries) {
    if (libraries.length < 2) { filterSlot.replaceChildren(); return; }
    const all = libraries.map((l) => l.id);
    const isOn = (lid) => !picked || picked.has(lid);
    const set = (next) => {
      picked = !next.size || next.size === all.length ? null : next;
      replaceQuery({ libraries: picked ? [...picked].join(',') : null });
      paintFilter(libraries);
      dv.load();
    };
    filterSlot.replaceChildren(h('div', { class: 'filters trail-filter', role: 'group', 'aria-label': 'Libraries' },
      h('span', { class: 'trail-filter-label' }, 'Libraries'),
      libraries.map((l) => h('button', { type: 'button', class: 'chip chip-btn chip-toggle', 'aria-pressed': String(isOn(l.id)),
        onClick: () => { const next = new Set(all.filter(isOn)); if (next.has(l.id)) next.delete(l.id); else next.add(l.id); set(next); } },
      isOn(l.id) ? icon('check', 12) : null, l.name)),
      picked ? h('button', { type: 'button', class: 'btn btn-ghost btn-sm', onClick: () => set(new Set(all)) }, 'Show all') : null));
  }

  /** The trail plus its "older" control; further pages are appended in place. */
  function trailView(first) {
    const trail = snake(ctx.signal);
    const foot = h('div', { class: 'trail-foot' });
    const box = h('div', null, trail.el, foot);
    let next = first.next, busy = false;

    const more = h('button', { type: 'button', class: 'btn', onClick: () => loadMore() }, 'Show older');
    const watcher = 'IntersectionObserver' in window ? new IntersectionObserver((e) => { if (e.some((x) => x.isIntersecting)) loadMore(); }, { rootMargin: '600px' }) : null;
    ctx.signal.addEventListener('abort', () => watcher && watcher.disconnect());

    function paintFoot(err) {
      if (watcher) watcher.disconnect();
      if (err) { foot.replaceChildren(errorState(err, () => loadMore())); return; }
      if (!next) { foot.replaceChildren(h('p', { class: 'trail-end' }, 'This is where the history starts.')); return; }
      foot.replaceChildren(more);
      if (watcher) watcher.observe(more);
    }

    async function loadMore() {
      if (busy || !next || !box.isConnected) return;
      busy = true;
      more.setAttribute('aria-busy', 'true');
      try {
        const d = await api.get(`/users/${id}/timeline`, params({ before: next }), { signal: ctx.signal });
        next = d.next;
        trail.add(d.stops, !next);
        paintFoot();
      } catch (err) {
        if (err.name !== 'AbortError') paintFoot(err);
      } finally {
        busy = false;
        more.removeAttribute('aria-busy');
      }
    }

    trail.add(first.stops, !next);
    paintFoot();
    return box;
  }

  headerSlot.append(h('header', { class: 'page-header entity-header' }, h('span', { class: 'sk', style: { width: '56px', height: '56px', borderRadius: '50%' } }), h('div', null, sk.line('180px', 26))));
  ctx.root.append(headerSlot, userTabs(id, 'timeline'), filterSlot, view);
  dv.load();
}

/** Lays stops out in rows that alternate direction. The row count follows the width; one column is a plain line. */
function snake(signal) {
  const el = h('div', { class: 'trail', role: 'list' });
  const cells = [];
  let cols = 0, ended = false, lastMonth = null;

  function layout(force) {
    const w = el.clientWidth;
    if (!w) return;
    const want = w < LINE_BELOW ? 1 : Math.max(2, Math.min(MAX_COLS, Math.floor(w / COL_MIN)));
    if (want === cols && !force) return;
    cols = want;
    el.classList.toggle('is-line', cols === 1);
    el.style.setProperty('--cols', cols);
    const rows = [];
    for (let i = 0; i < cells.length; i += cols) {
      rows.push(h('div', { class: ['trail-row', (i / cols) % 2 === 1 && cols > 1 && 'is-rev'], role: 'presentation' }, cells.slice(i, i + cols)));
    }
    el.replaceChildren(...rows);
  }

  if ('ResizeObserver' in window) {
    const ro = new ResizeObserver(() => layout(false));
    ro.observe(el);
    signal.addEventListener('abort', () => ro.disconnect());
  }

  return {
    el,
    add(stops, isEnd) {
      for (const s of stops) {
        const month = monthf.format(new Date(s.to * 1000));
        cells.push(stopCell(s, month !== lastMonth ? month : null));
        lastMonth = month;
      }
      ended = isEnd;
      cells.forEach((c, i) => { c.classList.toggle('is-first', i === 0); c.classList.toggle('is-last', ended && i === cells.length - 1); });
      if (!el.isConnected) requestAnimationFrame(() => layout(true)); else layout(true);
    },
  };
}

function stopCell(s, month) {
  const href = `/items/${s.id}`;
  return h('div', { class: ['stop', TYPE_CLASS[s.type] || 'is-other', month && 'has-month'], role: 'listitem' },
    month ? h('span', { class: 'stop-month' }, month) : h('span', { class: 'stop-node', 'aria-hidden': 'true' }),
    h('a', { class: 'stop-card', href },
      poster(s.image_item_id, s.name, { w: 160, cls: 'poster-md' }),
      h('span', { class: 'stop-text' },
        h('span', { class: 'stop-name' }, s.name),
        s.sub ? h('span', { class: 'stop-sub' }, s.sub) : null,
        h('span', { class: 'stop-what' }, what(s)),
        h('span', { class: 'stop-when' }, when(s),
          s.active ? h('span', { class: 'stop-now' }, 'Playing now') : h('span', { class: 'stop-dur', title: durationExact(s.watch_s) }, duration(s.watch_s))))));
}

/** "Episodes 3–6", "4 episodes", "7 tracks", "Watched in 2 sittings". */
function what(s) {
  const again = s.plays > s.titles ? ` · ${s.plays} plays` : '';
  if (s.kind === 'season') {
    if (s.episode_from != null) return (s.episode_from === s.episode_to ? `Episode ${s.episode_from}` : `Episodes ${s.episode_from}–${s.episode_to}`) + again;
    return (s.titles === 1 ? '1 episode' : `${s.titles} episodes`) + again;
  }
  if (s.kind === 'album') return (s.titles === 1 ? '1 track' : `${s.titles} tracks`) + again;
  if (s.type === 'Movie') return s.plays > 1 ? `Movie · watched in ${s.plays} sittings` : 'Movie';
  return s.plays > 1 ? `${s.plays} plays` : null;
}

/** One day: "19 Sep, 21:04". Several: "17 – 19 Sep". The year appears once it is not this one. */
function when(s) {
  const a = new Date(s.from * 1000), b = new Date(s.to * 1000);
  const f = b.getFullYear() === new Date().getFullYear() ? dayf : dayfy;
  const sameDay = a.toDateString() === b.toDateString();
  const text = sameDay ? `${f.format(a)}, ${timef.format(a)}` : `${(a.getFullYear() === b.getFullYear() ? dayf : dayfy).format(a)} – ${f.format(b)}`;
  return h('time', { dateTime: b.toISOString(), title: `${dateTime(s.from)} – ${dateTime(s.to)}` }, text);
}
