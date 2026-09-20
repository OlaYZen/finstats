import { h, icon, mount, relEl, episodeCode } from '../dom.js';
import { api, isAbort, soft } from '../api.js';
import { state, readDays, saveDays, can } from '../state.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, filterBar, dataView, sk, topList, playsTable, emptyState, poster } from '../components.js';
import { activityCard, heatmapCard, overviewTiles, nowPlayingView, insightTiles, genresCard, failedLoginsCard, shelfRow } from '../widgets.js';
import { openPlayModal } from '../playmodal.js';
import { recapBanner } from './recap.js';
import { loadUpcoming, entryCard } from '../upcoming.js';
import { groupsCard } from '../widgets.js';

// Coming up: the same loader for the card and the prefetcher. Nothing is asked when no Sonarr or Radarr is connected.
const hasComing = () => !!(state.user && state.user.features && state.user.features.upcoming);
const loadComing = (signal) => soft(loadUpcoming({ days: 7 }, signal));

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
export const prefetchDashboard = ({ query, signal }) => [() => loadDashboard(scopeOf(query), signal), () => loadRecent(signal), ...(hasComing() ? [() => loadComing(signal)] : [])];

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

  // ---- coming up: what Sonarr and Radarr expect this week. About the calendar, not about plays: no filter applies. Hides
  // itself when nothing is connected or nothing is due.
  const comingBody = h('div');
  const comingArrows = h('span', { class: 'shelf-arrows', hidden: true });
  const comingCard = card({ title: 'Coming up', sub: 'The next seven days in Sonarr and Radarr', cls: 'shelf-section', body: comingBody,
    actions: [comingArrows, h('a', { class: 'btn btn-ghost btn-sm', href: '/pipeline?tab=upcoming' }, 'Calendar')] });
  comingCard.hidden = true;
  if (hasComing()) {
    dataView({
      container: comingBody, signal: ctx.signal,
      skeleton: () => null,
      fetch: () => loadComing(ctx.signal),
      render: (d) => {
        const list = ((d && d.entries) || []).slice(0, 30);
        comingCard.hidden = !list.length;
        return list.length ? shelfRow(list.map(entryCard), comingArrows, 'Coming up, soonest first') : null;
      },
    }).load();
  }

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
          can('manage') ? h('a', { class: 'btn btn-primary', href: '/settings#import' }, icon('upload', 14), 'Import from Jellystat') : null), shelfCard, comingCard];
      }
      const usersMode = admin && !userId;
      return [
        overviewTiles({ totals: overview.totals, previous: overview.previous, daily: overview.daily, days, scoped: !admin || !!userId }),
        insightTiles(insights),
        shelfCard,
        comingCard,
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

/** Recently added: one card per arrival. */
function shelf(items, arrows) {
  return shelfRow(items.map((it) => h('a', { class: 'shelf-card', href: `/items/${it.id}` },
    poster(it.image_item_id || it.id, it.name, { w: 300, cls: 'poster-grid' }),
    h('span', { class: 'shelf-when' }, relEl(it.added_at, '')),
    h('span', { class: 'shelf-name' }, it.name),
    it.sub ? h('span', { class: 'shelf-sub' }, it.sub) : null,
    h('span', { class: 'shelf-sub' }, whatArrived(it)))), arrows, 'Recently added, newest first');
}

function whatArrived(it) {
  if (it.kind !== 'episodes') return { Movie: 'Movie', MusicAlbum: 'Album', MusicVideo: 'Music video', Video: 'Video', Book: 'Book', AudioBook: 'Audiobook' }[it.type] || it.type;
  if (it.episodes === 1) return it.episode_number != null ? `Episode ${it.episode_number}` : '1 episode';
  return `${it.episodes} episodes`;
}

