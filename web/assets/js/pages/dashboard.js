import { h, icon, mount } from '../dom.js';
import { api, isAbort, soft } from '../api.js';
import { state, isAdmin, readDays, saveDays } from '../state.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, filterBar, dataView, sk, topList, playsTable, emptyState } from '../components.js';
import { activityCard, heatmapCard, overviewTiles, nowPlayingList, insightTiles, genresCard, failedLoginsCard } from '../widgets.js';
import { openPlayModal } from '../playmodal.js';
import { recapBanner } from './recap.js';

export default function dashboard(ctx) {
  ctx.title('Dashboard');
  let days = readDays(ctx.query);
  let userId = isAdmin() ? ctx.query.get('user_id') || '' : '';
  const admin = isAdmin();

  // ---- now playing (live; not scoped by the filters below)
  const npCount = h('span', { class: 'count-pill mono', hidden: true });
  const npBody = h('div', { class: 'np-wrap' }, h('div', { class: 'np-grid' }, h('div', { class: 'np sk-np' }, h('span', { class: 'sk sk-poster' }),
    h('div', { class: 'sk-row-lines' }, sk.line('50%', 14), sk.line('35%'), sk.line('80%', 8)))));
  let npFirst = true;
  async function loadNow() {
    try {
      const data = await api.get('/now-playing', null, { signal: ctx.signal });
      const sessions = data.sessions || [];
      npCount.hidden = !sessions.length;
      npCount.textContent = String(sessions.length);
      mount(npBody, nowPlayingList(sessions));
      npFirst = false;
    } catch (e) {
      if (isAbort(e) || e.status === 401) return;
      if (npFirst) mount(npBody, h('p', { class: 'np-empty' }, icon('alert', 14), ' Live sessions are unavailable: ' + e.message));
    }
  }

  // ---- everything the filter row scopes
  const view = h('div', { class: 'stack' });
  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.tiles(4), sk.cardBlock(260), h('div', { class: 'grid-3' }, sk.cardRows(5), sk.cardRows(5), sk.cardRows(5))],
    fetch: async () => {
      const f = { days, user_id: userId };
      const o = { signal: ctx.signal };
      const [overview, series, movies, users, heat, recent, insights] = await Promise.all([
        api.get('/stats/overview', f, o),
        api.get('/stats/top', { ...f, kind: 'series', limit: 5 }, o),
        api.get('/stats/top', { ...f, kind: 'movies', limit: 5 }, o),
        admin && !userId ? api.get('/stats/top', { ...f, kind: 'users', limit: 5 }, o) : api.get('/stats/top', { ...f, kind: 'music', limit: 5 }, o),
        api.get('/stats/heatmap', f, o),
        api.get('/activity', { ...f, page: 1, per_page: 8 }, o),
        soft(api.get('/stats/insights', f, o)), // optional: the page works without it
      ]);
      return { overview, series, movies, users, heat, recent, insights };
    },
    render: ({ overview, series, movies, users, heat, recent, insights }) => {
      const nothingYet = days === 0 && !userId && !(overview.totals && overview.totals.plays);
      if (nothingYet) {
        return emptyState('No plays recorded yet',
          admin ? 'finstats is now watching your Jellyfin server — new plays show up here as they happen. You can also bring in your history from Jellystat.'
                : 'Your plays show up here as they happen.',
          admin ? h('a', { class: 'btn btn-primary', href: '/settings#import' }, icon('upload', 14), 'Import from Jellystat') : null);
      }
      const usersMode = admin && !userId;
      return [
        overviewTiles({ totals: overview.totals, previous: overview.previous, daily: overview.daily, days, scoped: !admin || !!userId }),
        insightTiles(insights),
        activityCard({ daily: overview.daily, bucket: overview.bucket }),
        h('div', { class: 'grid-3' },
          card({ title: 'Top series', sub: 'By watch time', body: topList(series.rows) }),
          card({ title: 'Top movies', sub: 'By watch time', body: topList(movies.rows) }),
          usersMode ? card({ title: 'Top users', sub: 'By watch time', body: topList(users.rows, { kind: 'users' }) })
                    : card({ title: 'Top music', sub: 'By watch time', body: topList(users.rows, { empty: 'No music played in this range.' }) })),
        insights ? h('div', { class: 'grid-heat' }, heatmapCard({ data: heat }), genresCard(insights.genres)) : heatmapCard({ data: heat }),
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
