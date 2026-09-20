import { h, icon, mount, relEl, episodeCode } from '../dom.js';
import { api, isAbort, soft } from '../api.js';
import { state, readDays, saveDays, can } from '../state.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, filterBar, dataView, sk, topList, playsTable, emptyState, poster } from '../components.js';
import { activityCard, heatmapCard, overviewTiles, nowPlayingView, insightTiles, genresCard, failedLoginsCard } from '../widgets.js';
import { openPlayModal } from '../playmodal.js';
import { recapBanner } from './recap.js';
import { groupsCard } from '../widgets.js';

/** Everything the filter row scopes. Shared with the prefetcher, so both ask for exactly the same URLs. */
export async function loadDashboard({ days, userId }, signal) {
  const f = { days, user_id: userId };
  const o = { signal };
  const admin = can('see_everyone');
  const [overview, series, movies, users, heat, recent, insights, groups] = await Promise.all([
    api.get('/stats/overview', f, o),
    api.get('/stats/top', { ...f, kind: 'series', limit: 5 }, o),
    api.get('/stats/top', { ...f, kind: 'movies', limit: 5 }, o),
    admin && !userId ? api.get('/stats/top', { ...f, kind: 'users', limit: 5 }, o) : api.get('/stats/top', { ...f, kind: 'music', limit: 5 }, o),
    api.get('/stats/heatmap', f, o),
    api.get('/activity', { ...f, page: 1, per_page: 8 }, o),
    soft(api.get('/stats/insights', f, o)), // optional: the page works without it
    soft(api.get('/stats/groups', f, o)),
  ]);
  return { overview, series, movies, users, heat, recent, insights, groups };
}
const scopeOf = (query) => ({ days: readDays(query), userId: can('see_everyone') ? query.get('user_id') || '' : '' });
/** Newest arrivals in the libraries: the same for every range and person, so it loads apart from the filters. */
const loadRecent = (signal) => soft(api.get('/library/recent', { limit: 30 }, { signal }));
export const prefetchDashboard = ({ query, signal }) => [() => loadDashboard(scopeOf(query), signal), () => loadRecent(signal)];

export default function dashboard(ctx) {
  ctx.title('Dashboard');
  let { days, userId } = scopeOf(ctx.query);
  const admin = can('see_everyone'); // sees the whole server rather than only themselves

  // ---- now playing (live; not scoped by the filters below)
  const npCount = h('span', { class: 'count-pill mono', hidden: true });
  const npBody = h('div', { class: 'np-wrap' }, h('div', { class: 'np-grid' }, h('div', { class: 'np sk-np' }, h('span', { class: 'sk sk-poster' }),
    h('div', { class: 'sk-row-lines' }, sk.line('50%', 14), sk.line('35%'), sk.line('80%', 8)))));
  let npFirst = true;
  const npLive = nowPlayingView(); // ticks every second between the polls
  ctx.onCleanup(() => npLive.destroy());
  async function loadNow() {
    try {
      const data = await api.get('/now-playing', null, { signal: ctx.signal });
      const sessions = data.sessions || [];
      npCount.hidden = !sessions.length;
      npCount.textContent = String(sessions.length);
      if (npFirst) mount(npBody, npLive.el);
      npLive.update(sessions);
      npFirst = false;
    } catch (e) {
      if (isAbort(e) || e.status === 401) return;
      if (npFirst) mount(npBody, h('p', { class: 'np-empty' }, icon('alert', 14), ' Live sessions are unavailable: ' + e.message));
    }
  }

  // ---- recently added: about the library, not the plays, so no filter applies and it loads on its own. The card is one node
  // that every render of the filtered view below slots in above the Activity chart. Hides itself on empty libraries.
  const shelfBody = h('div');
  const shelfArrows = h('span', { class: 'shelf-arrows', hidden: true });
  const shelfCard = card({ title: 'Recently added', sub: 'The newest in your libraries', cls: 'shelf-section', actions: shelfArrows, body: shelfBody });
  shelfCard.hidden = true;
  dataView({
    container: shelfBody, signal: ctx.signal,
    skeleton: () => null,
    fetch: () => loadRecent(ctx.signal),
    render: (d) => {
      const items = (d && d.items) || [];
      shelfCard.hidden = !items.length;
      return items.length ? shelf(items, shelfArrows) : null;
    },
  }).load();

  // ---- everything the filter row scopes
  const view = h('div', { class: 'stack' });
  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.tiles(4), sk.cardBlock(260), h('div', { class: 'grid-3' }, sk.cardRows(5), sk.cardRows(5), sk.cardRows(5))],
    fetch: () => loadDashboard({ days, userId }, ctx.signal),
    render: ({ overview, series, movies, users, heat, recent, insights, groups }) => {
      const nothingYet = days === 0 && !userId && !(overview.totals && overview.totals.plays);
      if (nothingYet) {
        return [emptyState('No plays recorded yet',
          can('manage') ? 'finstats is now watching your Jellyfin server — new plays show up here as they happen. You can also bring in your history from Jellystat.'
                : 'Your plays show up here as they happen.',
          can('manage') ? h('a', { class: 'btn btn-primary', href: '/settings#import' }, icon('upload', 14), 'Import from Jellystat') : null), shelfCard];
      }
      const usersMode = admin && !userId;
      return [
        overviewTiles({ totals: overview.totals, previous: overview.previous, daily: overview.daily, days, scoped: !admin || !!userId }),
        insightTiles(insights),
        shelfCard,
        activityCard({ daily: overview.daily, bucket: overview.bucket }),
        h('div', { class: 'grid-3' },
          card({ title: 'Top series', sub: 'By watch time', body: topList(series.rows) }),
          card({ title: 'Top movies', sub: 'By watch time', body: topList(movies.rows) }),
          usersMode ? card({ title: 'Top users', sub: 'By watch time', body: topList(users.rows, { kind: 'users' }) })
                    : card({ title: 'Top music', sub: 'By watch time', body: topList(users.rows, { empty: 'No music played in this range.' }) })),
        insights ? h('div', { class: 'grid-heat' }, heatmapCard({ data: heat }), genresCard(insights.genres)) : heatmapCard({ data: heat }),
        groupsCard(groups, { forUser: userId || (admin ? null : state.user.id) }),
        insights ? failedLoginsCard(insights.failed_logins) : null,
        card({ title: 'Recent activity', actions: h('a', { class: 'btn btn-ghost btn-sm', href: '/activity' }, 'View all', icon('chevronRight', 14)),
          cls: 'card-flush', body: playsTable(recent.rows, { showUser: admin, onOpen: (p) => openPlayModal(p, { onDeleted: () => dv.load() }), empty: 'No plays in this range.' }) }),
      ];
    },
  });

  const sync = () => replaceQuery({ days, user_id: userId });
  const filters = filterBar({ days, userId, signal: ctx.signal,
    onDays: (v) => { days = v; saveDays(v); sync(); dv.load(); },
    onUser: (v) => { userId = v; sync(); dv.load(); } });

  // Native append() prints a missing node as the text "null", so absent pieces are filtered out.
  ctx.root.append(...[
    pageHeader('Dashboard', state.status && state.status.server_name ? `Playback on ${state.status.server_name}` : 'Playback on your Jellyfin server'),
    recapBanner(),
    h('section', { class: 'np-section', 'aria-label': 'Now playing' }, h('h2', { class: 'section-title' }, 'Now playing', npCount), npBody),
    filters, view].filter(Boolean));

  loadNow();
  ctx.every(loadNow, 5000, { visibleOnly: true });
  dv.load();
}

/** A row of posters that scrolls sideways inside itself; the arrows (put into `arrows`) move it by most of a screen. */
function shelf(items, arrows) {
  const list = h('ul', { class: 'shelf', tabindex: 0, 'aria-label': 'Recently added, newest first' }, items.map((it) => h('li', null, h('a', { class: 'shelf-card', href: `/items/${it.id}` },
    poster(it.image_item_id || it.id, it.name, { w: 300, cls: 'poster-grid' }),
    h('span', { class: 'shelf-when' }, relEl(it.added_at, '')),
    h('span', { class: 'shelf-name' }, it.name),
    it.sub ? h('span', { class: 'shelf-sub' }, it.sub) : null,
    h('span', { class: 'shelf-sub' }, whatArrived(it))))));
  // Scrolling is done by hand. CSS scroll-snap swallowed a wheel notch that moved less than half a card (the row sprang back),
  // native arrow keys moved 40 px at a time and Home/End not at all. Everything goes through one glide towards a target, so
  // fast repeated input adds up instead of restarting an animation.
  const calm = window.matchMedia && matchMedia('(prefers-reduced-motion: reduce)').matches;
  let target = 0, frame = 0;
  const max = () => Math.max(0, list.scrollWidth - list.clientWidth);
  const step = () => { const li = list.firstElementChild; return li ? li.getBoundingClientRect().width + (parseFloat(getComputedStyle(list).columnGap) || 0) : 148; };
  function glide(to) {
    target = Math.max(0, Math.min(max(), to));
    if (calm) { list.scrollLeft = target; return; }
    if (frame) return;
    const tick = () => {
      const d = target - list.scrollLeft, before = list.scrollLeft;
      if (Math.abs(d) < 1.5) { list.scrollLeft = target; frame = 0; return; }
      list.scrollLeft = before + Math.sign(d) * Math.max(1, Math.abs(d) * 0.24);
      frame = list.scrollLeft === before ? 0 : requestAnimationFrame(tick); // an edge we cannot pass: stop
    };
    frame = requestAnimationFrame(tick);
  }
  const from = () => (frame ? target : list.scrollLeft); // mid-glide, the next input continues from where the glide is heading
  const page = (dir) => glide(from() + dir * Math.max(step(), list.clientWidth - step()));

  const prev = h('button', { type: 'button', class: 'icon-btn', 'aria-label': 'Scroll back', onClick: () => page(-1) }, icon('chevronLeft', 16));
  const next = h('button', { type: 'button', class: 'icon-btn', 'aria-label': 'Scroll on', onClick: () => page(1) }, icon('chevronRight', 16));
  arrows.replaceChildren(prev, next);

  // Shift + wheel (a mouse has no sideways wheel). A trackpad's own sideways swipe and the
  // plain wheel are left to the browser, so the page still scrolls normally with the pointer over the row.
  list.addEventListener('wheel', (e) => {
    if (!e.shiftKey || e.ctrlKey || e.metaKey) return;
    const raw = e.deltaX || e.deltaY; // Chromium turns shift+wheel into deltaX, Firefox leaves it in deltaY
    if (!raw || max() <= 0) return;
    e.preventDefault();
    const px = Math.abs(e.deltaMode === 1 ? raw * 16 : e.deltaMode === 2 ? raw * list.clientWidth : raw);
    // One notch is one poster, like one press of an arrow key, however many pixels the browser calls a notch (48 to 100). People
    // flick several notches at a time, so anything more per notch overshoots. The small deltas of a trackpad or a free-spinning
    // wheel scale instead, a notch's worth of them adding up to the same poster.
    glide(from() + Math.sign(raw) * (px >= 30 ? step() : px * step() / 50));
  }, { passive: false });

  // Arrow keys move one poster, Page Up/Down a screenful, Home/End to the ends — on the row itself or on a card in it.
  list.addEventListener('keydown', (e) => {
    if (e.altKey || e.ctrlKey || e.metaKey) return;
    const to = { ArrowRight: () => from() + step(), ArrowLeft: () => from() - step(), PageDown: () => from() + list.clientWidth - step(),
      PageUp: () => from() - list.clientWidth + step(), Home: () => 0, End: () => max() }[e.key];
    if (!to) return;
    e.preventDefault();
    glide(to());
  });

  // The arrows stay clickable at either end (aria-disabled says why nothing happens) and vanish when everything fits.
  const paint = () => {
    arrows.hidden = max() <= 4;
    prev.setAttribute('aria-disabled', String(list.scrollLeft <= 2));
    next.setAttribute('aria-disabled', String(list.scrollLeft >= max() - 2));
  };
  list.addEventListener('scroll', paint, { passive: true });
  if ('ResizeObserver' in window) new ResizeObserver(paint).observe(list);
  requestAnimationFrame(paint);
  return list;
}

function whatArrived(it) {
  if (it.kind !== 'episodes') return { Movie: 'Movie', MusicAlbum: 'Album', MusicVideo: 'Music video', Video: 'Video', Book: 'Book', AudioBook: 'Audiobook' }[it.type] || it.type;
  if (it.episodes === 1) return it.episode_number != null ? `Episode ${it.episode_number}` : '1 episode';
  return `${it.episodes} episodes`;
}

