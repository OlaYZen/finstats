import { h, icon, num, durEl, relEl, compact, duration, durationExact } from '../dom.js';
import { api, soft } from '../api.js';
import { readDays, saveDays, can, isAdmin, state } from '../state.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, filterBar, dataView, sk, avatar, emptyState, topList, chip, playsTable, statTile } from '../components.js';
import { activityCard, heatmapCard, genresCard, groupsCard } from '../widgets.js';
import { bucketList, methodsBar } from '../charts.js';
import { openPlayModal } from '../playmodal.js';
import { profileAllTime } from './showprogress.js';
import { dataTable, plainTable } from '../tables.js';

// Loaders are shared with the prefetcher, so a prefetched view has exactly the address the page asks for.
const loadUsers = (days, signal) => api.get('/users', { days }, { signal });
async function loadUser(id, days, signal) {
  const o = { signal };
  const [detail, recent, groups] = await Promise.all([
    api.get(`/users/${id}`, { days }, o),
    api.get('/activity', { days, user_id: id, page: 1, per_page: 10 }, o),
    soft(api.get('/stats/groups', { days, user_id: id }, o)),
  ]);
  return { detail, recent, groups };
}
export const prefetchUsers = ({ query, signal }) => [() => loadUsers(readDays(query), signal)];
export const prefetchUser = ({ params, query, signal }) => [() => loadUser(params.id, readDays(query), signal)];

// ---------------------------------------------------------------- /users
export function usersPage(ctx) {
  ctx.title('Users');
  let days = readDays(ctx.query);
  const view = h('div');
  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => sk.rows(5),
    fetch: () => loadUsers(days, ctx.signal),
    render: (data) => {
      const users = (data.users || []).slice().sort((a, b) => (b.watch_s || 0) - (a.watch_s || 0));
      if (!users.length) return emptyState('No users yet', 'Users appear after the first sync with Jellyfin.');
      return dataTable(h('table', { class: 'table table-hover' },
        h('thead', null, h('tr', null, h('th', null, 'User'), h('th', { class: 'r' }, 'Plays'), h('th', { class: 'r' }, 'Watch time'),
          h('th', null, 'Last played'), h('th', null, 'Last title'), h('th', null, 'Last client'), h('th', null, 'Last seen on Jellyfin'))),
        h('tbody', null, users.map((u) => h('tr', { class: u.removed || u.is_disabled ? 'is-dim' : '' },
          h('td', null, h('a', { class: 'user-cell user-cell-lg', href: `/users/${u.id}` }, avatar(u.id, u.name, { size: 30, hasImage: u.has_image }),
            h('span', { class: 'user-ident' }, h('span', { class: 'user-name' }, u.name),
              h('span', { class: 'user-tags' }, u.is_admin ? chip('Admin') : null, u.is_disabled ? chip('Disabled') : null, u.removed ? chip('Removed from Jellyfin') : null)))),
          h('td', { class: 'mono r' }, num(u.plays)),
          h('td', { class: 'r' }, durEl(u.watch_s)),
          h('td', null, u.last_played_at ? relEl(u.last_played_at) : h('span', { class: 'muted' }, 'Never')),
          h('td', { class: 'trunc-cell' }, u.last_item_name || h('span', { class: 'muted' }, '–')),
          h('td', null, u.last_client || h('span', { class: 'muted' }, '–')),
          h('td', null, u.last_activity_at ? relEl(u.last_activity_at) : h('span', { class: 'muted' }, '–')))))));
    },
  });
  ctx.root.append(pageHeader('Users', 'Everyone with an account on your Jellyfin server'),
    filterBar({ days, onDays: (v) => { days = v; saveDays(v); replaceQuery({ days }); dv.load(); } }),
    card({ cls: 'card-flush', body: view }));
  dv.load();
}

/** Overview | Timeline, under a person's header. Links, so each view has its own address. */
export function userTabs(id, current) {
  const tab = (key, label, href) => h('a', { class: 'seg-btn', href, 'aria-current': key === current ? 'page' : null }, label);
  return h('nav', { class: 'seg entity-tabs', 'aria-label': 'Profile sections' },
    tab('overview', 'Overview', `/users/${id}`), tab('timeline', 'Timeline', `/users/${id}/timeline`));
}

// ---------------------------------------------------------------- /users/:id
export function userPage(ctx) {
  const id = ctx.params.id;
  ctx.title('User');
  let days = readDays(ctx.query);
  const headerSlot = h('div');
  const view = h('div', { class: 'stack' });
  const allTime = profileAllTime({ userId: id, signal: ctx.signal });

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.tiles(4), sk.cardBlock(260), h('div', { class: 'grid-2' }, sk.cardRows(4), sk.cardRows(4))],
    fetch: () => loadUser(id, days, ctx.signal),
    render: ({ detail: d, recent, groups }) => {
      const u = d.user;
      ctx.title(u.name);
      headerSlot.replaceChildren(h('header', { class: 'page-header entity-header' },
        avatar(u.id, u.name, { size: 56, hasImage: u.has_image }),
        h('div', null, h('h1', { class: 'page-title' }, u.name),
          isAdmin() && state.user && u.id !== state.user.id ? h('a', { class: 'btn btn-ghost btn-sm entity-action', href: `/recap?user=${encodeURIComponent(u.id)}` }, icon('recap', 14), 'Open recap') : null,
          h('p', { class: 'page-sub' }, [u.is_admin ? 'Administrator' : 'User', u.is_disabled ? 'disabled' : null, u.removed ? 'removed from Jellyfin' : null].filter(Boolean).join(' · '),
            u.last_activity_at ? [' · last seen ', relEl(u.last_activity_at, '')] : null))));
      const t = d.totals || {};
      return [
        h('div', { class: 'tiles' },
          statTile({ label: 'Watch time', value: duration(t.watch_s), title: durationExact(t.watch_s), hint: ' ' }),
          statTile({ label: 'Plays', value: compact(t.plays), title: num(t.plays), hint: `${num(t.distinct_items)} different titles` }),
          statTile({ label: 'Movies', value: compact(t.movies), hint: 'plays' }),
          statTile({ label: 'Episodes', value: compact(t.episodes), hint: t.tracks ? `and ${num(t.tracks)} music plays` : 'plays' })),
        jellyfinStrip(d.jellyfin),
        activityCard({ daily: d.daily, bucket: d.bucket }),
        h('div', { class: 'grid-2' },
          card({ title: 'Top series', sub: 'By watch time', body: topList(d.top_series) }),
          card({ title: 'Top movies', sub: 'By watch time', body: topList(d.top_movies) })),
        heatmapCard({ data: d.heatmap, title: `When ${u.name} watches` }),
        h('div', { class: 'grid-2' },
          card({ title: 'Play methods', sub: 'Share of plays', body: methodsBar(d.methods) }),
          card({ title: 'Clients', sub: 'By plays', body: bucketList(d.clients) })),
        allTime.shows,
        Array.isArray(d.genres) ? genresCard(d.genres) : null,
        groupsCard(groups, { forUser: id }),
        card({ title: 'Devices', cls: 'card-flush', body: devicesTable(d.devices) }),
        can('see_network') ? card({ title: 'IP addresses', sub: 'Where this user has played from', cls: 'card-flush', body: ipsTable(d.ips) }) : null,
        card({ title: 'Recent plays', cls: 'card-flush',
          actions: h('a', { class: 'btn btn-ghost btn-sm', href: `/activity?user_id=${encodeURIComponent(id)}&days=${days}` }, 'View all'),
          body: playsTable(recent.rows, { showUser: false, onOpen: (p) => openPlayModal(p, { onDeleted: () => dv.load() }), empty: 'No plays in this range.' }) }),
      ];
    },
  });

  // Streaks and show progress cover all time and load on their own. The streaks sit under the header;
  // the shows card is slotted into the page further down (the same node on every re-render).
  ctx.root.append(headerSlot, userTabs(id, 'overview'), allTime.tiles, filterBar({ days, onDays: (v) => { days = v; saveDays(v); replaceQuery({ days }); dv.load(); } }), view);
  headerSlot.append(h('header', { class: 'page-header entity-header' }, h('span', { class: 'sk', style: { width: '56px', height: '56px', borderRadius: '50%' } }), h('div', null, sk.line('180px', 26))));
  dv.load();
}

/** Jellyfin's own flags — they cover history from before finstats was installed. */
function jellyfinStrip(j) {
  if (!j) return null;
  const cell = (k, v) => h('div', null, h('dt', null, k), h('dd', { class: 'mono' }, num(v)));
  return h('section', { class: 'strip', 'aria-label': 'In Jellyfin' },
    h('h2', { class: 'strip-title' }, 'In Jellyfin'),
    h('dl', { class: 'strip-facts' }, cell('Movies played', j.played_movies), cell('Episodes played', j.played_episodes), cell('Favourites', j.favorites)),
    h('p', { class: 'strip-note' }, 'From Jellyfin’s played flags, including history from before finstats.'));
}

function devicesTable(devices) {
  if (!devices || !devices.length) return emptyState('No devices in this range.');
  return plainTable(h('table', { class: 'table' },
    h('thead', null, h('tr', null, h('th', null, 'Device'), h('th', null, 'Client'), h('th', null, 'Version'), h('th', { class: 'r' }, 'Plays'), h('th', null, 'Last used'))),
    h('tbody', null, devices.map((d) => h('tr', null,
      h('td', null, d.device_name || '–'), h('td', null, d.client || '–'), h('td', { class: 'mono' }, d.app_version || '–'),
      h('td', { class: 'mono r' }, num(d.plays)), h('td', null, relEl(d.last_seen)))))));
}

function ipsTable(ips) {
  if (!ips || !ips.length) return emptyState('No IP addresses recorded in this range.');
  return plainTable(h('table', { class: 'table' },
    h('thead', null, h('tr', null, h('th', null, 'IP address'), h('th', null, 'Network'), h('th', { class: 'r' }, 'Plays'), h('th', null, 'First seen'), h('th', null, 'Last seen'))),
    h('tbody', null, ips.map((ip) => h('tr', null,
      h('td', { class: 'mono' }, ip.ip), h('td', null, chip(ip.is_local ? 'Local' : 'Remote')),
      h('td', { class: 'mono r' }, num(ip.plays)), h('td', null, relEl(ip.first_seen)), h('td', null, relEl(ip.last_seen)))))));
}
