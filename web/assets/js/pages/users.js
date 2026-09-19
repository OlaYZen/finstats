import { h, num, durEl, relEl, compact, duration, durationExact } from '../dom.js';
import { api } from '../api.js';
import { isAdmin, readDays, saveDays } from '../state.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, filterBar, dataView, sk, avatar, emptyState, topList, chip, playsTable, statTile } from '../components.js';
import { activityCard, heatmapCard } from '../widgets.js';
import { bucketList, methodsBar } from '../charts.js';
import { openPlayModal } from '../playmodal.js';

// ---------------------------------------------------------------- /users
export function usersPage(ctx) {
  ctx.title('Users');
  let days = readDays(ctx.query);
  const view = h('div');
  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => sk.rows(5),
    fetch: () => api.get('/users', { days }, { signal: ctx.signal }),
    render: (data) => {
      const users = (data.users || []).slice().sort((a, b) => (b.watch_s || 0) - (a.watch_s || 0));
      if (!users.length) return emptyState('No users yet', 'Users appear after the first sync with Jellyfin.');
      return h('div', { class: 'table-scroll' }, h('table', { class: 'table table-hover' },
        h('thead', null, h('tr', null, h('th', null, 'User'), h('th', { class: 'r' }, 'Plays'), h('th', { class: 'r' }, 'Watch time'),
          h('th', null, 'Last played'), h('th', null, 'Last title'), h('th', null, 'Last client'), h('th', null, 'Last seen on Jellyfin'))),
        h('tbody', null, users.map((u) => h('tr', { class: u.removed || u.is_disabled ? 'is-dim' : '' },
          h('td', null, h('a', { class: 'user-cell user-cell-lg', href: `/users/${u.id}` }, avatar(u.id, u.name, { size: 30, hasImage: u.has_image }),
            h('span', null, h('span', { class: 'user-name' }, u.name),
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

// ---------------------------------------------------------------- /users/:id
export function userPage(ctx) {
  const id = ctx.params.id;
  ctx.title('User');
  let days = readDays(ctx.query);
  const headerSlot = h('div');
  const view = h('div', { class: 'stack' });

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.tiles(4), sk.cardBlock(260), h('div', { class: 'grid-2' }, sk.cardRows(4), sk.cardRows(4))],
    fetch: async () => {
      const o = { signal: ctx.signal };
      const [detail, recent] = await Promise.all([
        api.get(`/users/${id}`, { days }, o),
        api.get('/activity', { days, user_id: id, page: 1, per_page: 10 }, o),
      ]);
      return { detail, recent };
    },
    render: ({ detail: d, recent }) => {
      const u = d.user;
      ctx.title(u.name);
      headerSlot.replaceChildren(h('header', { class: 'page-header entity-header' },
        avatar(u.id, u.name, { size: 56, hasImage: u.has_image }),
        h('div', null, h('h1', { class: 'page-title' }, u.name),
          h('p', { class: 'page-sub' }, [u.is_admin ? 'Administrator' : 'User', u.is_disabled ? 'disabled' : null, u.removed ? 'removed from Jellyfin' : null].filter(Boolean).join(' · '),
            u.last_activity_at ? [' · last seen ', relEl(u.last_activity_at, '')] : null))));
      const t = d.totals || {};
      return [
        h('div', { class: 'tiles' },
          statTile({ label: 'Watch time', value: duration(t.watch_s), title: durationExact(t.watch_s), hint: ' ' }),
          statTile({ label: 'Plays', value: compact(t.plays), title: num(t.plays), hint: `${num(t.distinct_items)} different titles` }),
          statTile({ label: 'Movies', value: compact(t.movies), hint: 'plays' }),
          statTile({ label: 'Episodes', value: compact(t.episodes), hint: t.tracks ? `and ${num(t.tracks)} music plays` : 'plays' })),
        activityCard({ daily: d.daily, bucket: d.bucket }),
        h('div', { class: 'grid-2' },
          card({ title: 'Top series', sub: 'By watch time', body: topList(d.top_series) }),
          card({ title: 'Top movies', sub: 'By watch time', body: topList(d.top_movies) })),
        heatmapCard({ data: d.heatmap, title: `When ${u.name} watches` }),
        h('div', { class: 'grid-2' },
          card({ title: 'Play methods', sub: 'Share of plays', body: methodsBar(d.methods) }),
          card({ title: 'Clients', sub: 'By plays', body: bucketList(d.clients) })),
        card({ title: 'Devices', cls: 'card-flush', body: devicesTable(d.devices) }),
        isAdmin() ? card({ title: 'IP addresses', sub: 'Where this user has played from', cls: 'card-flush', body: ipsTable(d.ips) }) : null,
        card({ title: 'Recent plays', cls: 'card-flush',
          actions: h('a', { class: 'btn btn-ghost btn-sm', href: `/activity?user_id=${encodeURIComponent(id)}&days=${days}` }, 'View all'),
          body: playsTable(recent.rows, { showUser: false, onOpen: (p) => openPlayModal(p, { onDeleted: () => dv.load() }), empty: 'No plays in this range.' }) }),
      ];
    },
  });

  ctx.root.append(headerSlot, filterBar({ days, onDays: (v) => { days = v; saveDays(v); replaceQuery({ days }); dv.load(); } }), view);
  headerSlot.append(h('header', { class: 'page-header entity-header' }, h('span', { class: 'sk', style: { width: '56px', height: '56px', borderRadius: '50%' } }), h('div', null, sk.line('180px', 26))));
  dv.load();
}

function devicesTable(devices) {
  if (!devices || !devices.length) return emptyState('No devices in this range.');
  return h('div', { class: 'table-scroll' }, h('table', { class: 'table' },
    h('thead', null, h('tr', null, h('th', null, 'Device'), h('th', null, 'Client'), h('th', null, 'Version'), h('th', { class: 'r' }, 'Plays'), h('th', null, 'Last used'))),
    h('tbody', null, devices.map((d) => h('tr', null,
      h('td', null, d.device_name || '–'), h('td', null, d.client || '–'), h('td', { class: 'mono' }, d.app_version || '–'),
      h('td', { class: 'mono r' }, num(d.plays)), h('td', null, relEl(d.last_seen)))))));
}

function ipsTable(ips) {
  if (!ips || !ips.length) return emptyState('No IP addresses recorded in this range.');
  return h('div', { class: 'table-scroll' }, h('table', { class: 'table' },
    h('thead', null, h('tr', null, h('th', null, 'IP address'), h('th', null, 'Network'), h('th', { class: 'r' }, 'Plays'), h('th', null, 'First seen'), h('th', null, 'Last seen'))),
    h('tbody', null, ips.map((ip) => h('tr', null,
      h('td', { class: 'mono' }, ip.ip), h('td', null, chip(ip.is_local ? 'Local' : 'Remote')),
      h('td', { class: 'mono r' }, num(ip.plays)), h('td', null, relEl(ip.first_seen)), h('td', null, relEl(ip.last_seen)))))));
}
