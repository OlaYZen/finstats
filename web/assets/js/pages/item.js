import { h, icon, num, bytes, bitrate, duration, durationExact, durEl, relEl, dateTime, episodeCode, compact, safeHttps } from '../dom.js';
import { api, imgItem } from '../api.js';
import { isAdmin, readDays, saveDays } from '../state.js';
import { replaceQuery } from '../router.js';
import { card, filterBar, dataView, sk, poster, avatar, chip, statTile, playsTable, emptyState, facts } from '../components.js';
import { activityCard } from '../widgets.js';
import { openPlayModal } from '../playmodal.js';

const TYPE_LABEL = { Movie: 'Movie', Series: 'Series', Episode: 'Episode', Season: 'Season', Audio: 'Track', MusicAlbum: 'Album' };

export default function itemPage(ctx) {
  const id = ctx.params.id;
  ctx.title('Title');
  let days = readDays(ctx.query);
  const view = h('div', { class: 'stack' });
  const filtersSlot = h('div');

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [h('div', { class: 'item-hero' }, h('span', { class: 'sk sk-poster-lg' }), h('div', { class: 'sk-row-lines' }, sk.line('40%', 28), sk.line('60%'), sk.line('30%'))), sk.tiles(3), sk.cardBlock(240)],
    fetch: async () => {
      const o = { signal: ctx.signal };
      const d = await api.get(`/items/${id}`, { days }, o);
      const key = d.item.type === 'Series' ? 'series_id' : 'item_id';
      const recent = await api.get('/activity', { days, [key]: id, page: 1, per_page: 10 }, o);
      return { d, recent, key };
    },
    render: ({ d, recent, key }) => {
      const it = d.item, t = d.totals || {};
      ctx.title(it.name);
      const code = episodeCode(it.season_number, it.episode_number);
      const meta = [
        chip(TYPE_LABEL[it.type] || it.type),
        it.year ? chip(String(it.year)) : null,
        it.official_rating ? chip(it.official_rating) : null,
        it.community_rating ? chip('★ ' + Number(it.community_rating).toFixed(1), 'Community rating') : null,
        it.runtime_s ? chip(duration(it.runtime_s), 'Runtime') : null,
        it.video ? h('span', { class: 'chip mono' }, it.video) : null,
        it.audio ? h('span', { class: 'chip mono' }, it.audio) : null,
        it.removed ? chip('No longer in library') : null,
      ];
      const hero = h('div', { class: ['item-hero', it.has_backdrop && 'has-backdrop'] },
        it.has_backdrop ? h('div', { class: 'item-backdrop', 'aria-hidden': 'true' }, h('img', { src: imgItem(it.id, 1280, 'backdrop'), alt: '', decoding: 'async', onError: (e) => e.target.parentNode.remove() })) : null,
        poster(it.type === 'Episode' && it.series_id ? it.series_id : it.id, it.name, { w: 300, cls: 'poster-lg' }),
        h('div', { class: 'item-hero-text' },
          it.series_name && it.series_id ? h('a', { class: 'item-series', href: `/items/${it.series_id}` }, it.series_name, code ? h('span', { class: 'mono' }, ' · ' + code) : null) : null,
          h('h1', { class: 'page-title' }, it.name),
          h('div', { class: 'chips' }, meta),
          it.genres && it.genres.length ? h('p', { class: 'item-genres' }, it.genres.join(' · ')) : null,
          Array.isArray(it.studios) && it.studios.length ? h('p', { class: 'item-studios' }, it.studios.slice(0, 4).join(' · ')) : null,
          externalLinks(it.external),
          it.overview ? h('p', { class: 'item-overview' }, it.overview) : null,
          it.library_id ? h('p', { class: 'item-lib' }, 'In ', h('a', { href: `/libraries/${it.library_id}` }, it.library_name || 'library'),
            it.date_created ? [' · added ', h('span', { title: dateTime(it.date_created) }, relEl(it.date_created, ''))] : null) : null));

      const file = facts([
        it.container ? ['Container', it.container, { mono: true }] : null,
        it.size_bytes ? ['Size', bytes(it.size_bytes), { mono: true }] : null,
        it.bitrate ? ['Bitrate', bitrate(it.bitrate), { mono: true }] : null,
        it.bit_depth ? ['Bit depth', it.bit_depth + '-bit', { mono: true }] : null,
        it.framerate ? ['Frame rate', (Math.round(Number(it.framerate) * 1000) / 1000) + ' fps', { mono: true }] : null,
        isAdmin() && it.path ? ['Path', h('span', { class: 'mono path' }, it.path)] : null,
      ]);

      return [
        hero,
        h('div', { class: 'tiles tiles-3' },
          statTile({ label: 'Watch time', value: duration(t.watch_s), title: durationExact(t.watch_s), hint: ' ' }),
          statTile({ label: 'Plays', value: compact(t.plays), title: num(t.plays), hint: t.last_played_at ? ['last ', relEl(t.last_played_at, '')] : 'Never played' }),
          statTile({ label: 'Watched by', value: `${num(t.users)} ${t.users === 1 ? 'user' : 'users'}`, hint: ' ' })),
        activityCard({ daily: d.daily, bucket: d.bucket, title: 'Plays over time' }),
        d.seasons && d.seasons.length ? card({ title: 'Seasons', sub: 'Plays per episode in this range', body: seasons(d.seasons) }) : null,
        card({ title: 'Watched by', cls: 'card-flush', body: watchers(d.watchers) }),
        playedBy(d.played_by),
        card({ title: 'Recent plays', cls: 'card-flush',
          actions: h('a', { class: 'btn btn-ghost btn-sm', href: `/activity?${key}=${encodeURIComponent(id)}&days=${days}` }, 'View all'),
          body: playsTable(recent.rows, { showUser: isAdmin(), onOpen: (p) => openPlayModal(p, { onDeleted: () => dv.load() }), empty: 'No plays in this range.' }) }),
        file.children.length ? card({ title: 'File', body: file }) : null,
      ];
    },
  });

  filtersSlot.append(filterBar({ days, onDays: (v) => { days = v; saveDays(v); replaceQuery({ days }); dv.load(); } }));
  ctx.root.append(filtersSlot, view);
  dv.load();
}

function externalLinks(list) {
  const links = (Array.isArray(list) ? list : []).map((x) => ({ label: x && x.label, url: safeHttps(x && x.url) })).filter((x) => x.url && x.label);
  if (!links.length) return null;
  return h('div', { class: 'chips' }, links.map((x) =>
    h('a', { class: 'chip chip-link', href: x.url, target: '_blank', rel: 'noopener noreferrer' }, x.label, icon('external', 11), h('span', { class: 'sr-only' }, ' (opens in a new tab)'))));
}

/** Jellyfin's played flags: covers people who watched before finstats existed. */
function playedBy(rows) {
  if (!Array.isArray(rows) || !rows.length) return null;
  return card({ title: 'Marked played in Jellyfin', sub: 'Jellyfin’s own flags, including history from before finstats', cls: 'card-flush',
    body: h('div', { class: 'table-scroll' }, h('table', { class: 'table' },
      h('thead', null, h('tr', null, h('th', null, 'User'), h('th', null, 'Last played'), h('th', null, h('span', { class: 'sr-only' }, 'Favourite')))),
      h('tbody', null, rows.map((r) => h('tr', null,
        h('td', null, h('a', { class: 'user-cell', href: `/users/${r.user_id}` }, avatar(r.user_id, r.user_name, { size: 22 }), h('span', null, r.user_name || 'Unknown user'))),
        h('td', null, r.last_played_at ? relEl(r.last_played_at) : h('span', { class: 'muted' }, '–')),
        h('td', null, r.is_favorite ? h('span', { class: 'sev fav' }, icon('heart', 13), 'Favourite') : null)))))) });
}

function watchers(rows) {
  if (!rows || !rows.length) return emptyState('Nobody has played this in the selected range.');
  return h('div', { class: 'table-scroll' }, h('table', { class: 'table' },
    h('thead', null, h('tr', null, h('th', null, 'User'), h('th', { class: 'r' }, 'Plays'), h('th', { class: 'r' }, 'Watch time'), h('th', null, 'Last played'))),
    h('tbody', null, rows.map((w) => h('tr', null,
      h('td', null, h('a', { class: 'user-cell', href: `/users/${w.user_id}` }, avatar(w.user_id, w.user_name, { size: 22 }), h('span', null, w.user_name))),
      h('td', { class: 'mono r' }, num(w.plays)), h('td', { class: 'r' }, durEl(w.watch_s)), h('td', null, relEl(w.last_played_at)))))));
}

function seasons(list) {
  return h('div', { class: 'seasons' }, list.map((sn, i) => {
    const plays = sn.episodes.reduce((a, e) => a + (e.plays || 0), 0);
    const max = Math.max(1, ...sn.episodes.map((e) => e.plays || 0));
    const det = h('details', { class: 'season' },
      h('summary', null, icon('chevronRight', 14), h('span', { class: 'season-name' }, sn.name || `Season ${sn.season_number}`),
        h('span', { class: 'season-meta mono' }, `${num(sn.episodes.length)} ep · ${num(plays)} ${plays === 1 ? 'play' : 'plays'}`)),
      h('table', { class: 'table table-dense episodes' },
        h('thead', { class: 'sr-only' }, h('tr', null, h('th', null, '#'), h('th', null, 'Episode'), h('th', null, 'Share'), h('th', null, 'Plays'), h('th', null, 'Watch time'))),
        h('tbody', null, sn.episodes.map((e) => h('tr', null,
          h('td', { class: 'mono muted ep-num' }, e.episode_number != null ? String(e.episode_number) : '–'),
          h('td', null, h('a', { href: `/items/${e.id}` }, e.name)),
          h('td', { class: 'bucket-bar' }, h('span', { class: 'bucket-track' }, e.plays ? h('span', { class: 'bucket-fill', style: { width: Math.max(2, (e.plays / max) * 100) + '%' } }) : null)),
          h('td', { class: 'mono r' }, e.plays ? num(e.plays) : h('span', { class: 'muted' }, '0')),
          h('td', { class: 'mono r' }, e.watch_s ? duration(e.watch_s) : h('span', { class: 'muted' }, '–')))))));
    if (i === 0 && list.length === 1) det.open = true;
    return det;
  }));
}
