import { h, icon, num, bytes, durEl, relEl, relTime, dateTime } from '../dom.js';
import { api, soft, imgItem } from '../api.js';
import { readDays, saveDays } from '../state.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, filterBar, dataView, sk, emptyState, topList, poster } from '../components.js';
import { activityCard, libraryInsights } from '../widgets.js';

const KIND = { movies: 'Movies', tvshows: 'Shows', music: 'Music', musicvideos: 'Music videos', homevideos: 'Home videos', books: 'Books', boxsets: 'Collections', mixed: 'Mixed' };
const KIND_ICON = { movies: 'film', tvshows: 'play', music: 'activity' };

function counts(l) {
  if (l.collection_type === 'tvshows') return `${num(l.series_count)} series · ${num(l.episode_count)} episodes`;
  if (l.collection_type === 'music') return `${num(l.item_count)} tracks`;
  return `${num(l.item_count)} ${l.item_count === 1 ? 'item' : 'items'}`;
}

// ---------------------------------------------------------------- /libraries
/**
 * The picture Jellyfin shows for a library on its home screen. `fallback` (the plain type icon on
 * the cards) takes its place when Jellyfin has none; without a fallback it simply disappears.
 */
function libraryArt(l, w, cls, fallback = null) {
  if (l.removed) return fallback; // Jellyfin no longer has it, so there is nothing to ask for
  return h('img', { class: cls, src: imgItem(l.id, w), alt: '', loading: 'lazy', decoding: 'async',
    onError: (e) => { if (fallback) e.target.replaceWith(fallback); else e.target.remove(); } });
}

export function librariesPage(ctx) {
  ctx.title('Libraries');
  let days = readDays(ctx.query);
  const view = h('div');
  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => h('div', { class: 'lib-grid' }, [0, 1, 2].map(() => h('div', { class: 'lib-card' }, sk.line('40%', 16), sk.line('60%'), sk.block(48)))),
    fetch: () => api.get('/libraries', { days }, { signal: ctx.signal }),
    render: (data) => {
      const libs = (data.libraries || []).slice().sort((a, b) => (a.removed - b.removed) || (b.watch_s || 0) - (a.watch_s || 0));
      if (!libs.length) return emptyState('No libraries yet', 'Libraries appear after the first sync with Jellyfin. You can start one from Settings → Tasks.');
      return h('div', { class: 'lib-grid' }, libs.map((l) => h('a', { class: ['lib-card', l.removed && 'is-dim'], href: `/libraries/${l.id}` },
        h('div', { class: 'lib-head' }, libraryArt(l, 300, 'lib-art', h('span', { class: 'lib-icon' }, icon(KIND_ICON[l.collection_type] || 'library', 16))),
          h('div', null, h('div', { class: 'lib-name' }, l.name), h('div', { class: 'lib-kind' }, KIND[l.collection_type] || 'Library', l.removed ? ' · removed from Jellyfin' : ''))),
        h('div', { class: 'lib-counts mono' }, counts(l), l.size_bytes ? ` · ${bytes(l.size_bytes)}` : ''),
        h('dl', { class: 'lib-stats' },
          h('div', null, h('dt', null, 'Watch time'), h('dd', null, durEl(l.watch_s))),
          h('div', null, h('dt', null, 'Plays'), h('dd', { class: 'mono' }, num(l.plays))),
          h('div', null, h('dt', null, 'Last played'), h('dd', { class: 'mono', title: l.last_played_at ? dateTime(l.last_played_at) : '' }, l.last_played_at ? relTime(l.last_played_at) : 'Never'))))));
    },
  });
  ctx.root.append(pageHeader('Libraries', 'What’s on the server and how much of it gets watched'),
    filterBar({ days, onDays: (v) => { days = v; saveDays(v); replaceQuery({ days }); dv.load(); } }), view, makeupSlot(ctx));
  dv.load();
}

/** Library make-up loads once per page: it has no time range, so range changes never refetch it. */
function makeupSlot(ctx, libraryId) {
  const slot = h('div', { class: 'makeup' });
  dataView({
    container: slot, signal: ctx.signal,
    skeleton: () => [sk.line('240px', 18), sk.tiles(3), h('div', { class: 'grid-3' }, sk.cardRows(4), sk.cardRows(4), sk.cardRows(4))],
    fetch: () => soft(api.get('/library/insights', libraryId ? { library_id: libraryId } : null, { signal: ctx.signal })),
    render: (d) => libraryInsights(d, { scoped: !!libraryId }),
  }).load();
  return slot;
}

// ---------------------------------------------------------------- /libraries/:id
export function libraryPage(ctx) {
  const id = ctx.params.id;
  ctx.title('Library');
  let days = readDays(ctx.query);
  const headerSlot = h('div', null, pageHeader(sk.line('200px', 26)));
  const view = h('div', { class: 'stack' });
  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.cardBlock(260), sk.cardRows(5)],
    fetch: () => api.get(`/libraries/${id}`, { days }, { signal: ctx.signal }),
    render: (d) => {
      const l = d.library;
      ctx.title(l.name);
      headerSlot.replaceChildren(pageHeader(l.name, [KIND[l.collection_type] || 'Library', counts(l), l.size_bytes ? bytes(l.size_bytes) : null, l.removed ? 'removed from Jellyfin' : null].filter(Boolean).join(' · '), libraryArt(l, 480, 'lib-art lib-art-lg')));
      return [
        h('div', { class: 'tiles tiles-3' },
          h('div', { class: 'tile' }, h('div', { class: 'tile-label' }, 'Watch time'), h('div', { class: 'tile-value' }, durEl(l.watch_s, ''))),
          h('div', { class: 'tile' }, h('div', { class: 'tile-label' }, 'Plays'), h('div', { class: 'tile-value' }, num(l.plays))),
          h('div', { class: 'tile' }, h('div', { class: 'tile-label' }, 'Last played'), h('div', { class: 'tile-value' }, l.last_played_at ? relEl(l.last_played_at, '') : 'Never'))),
        activityCard({ daily: d.daily, bucket: d.bucket }),
        card({ title: 'Most watched', sub: 'By watch time', body: topList(d.top) }),
        card({ title: 'Recently added', body: itemGrid(d.recently_added) }),
      ];
    },
  });
  ctx.root.append(h('a', { class: 'back-link', href: '/libraries' }, icon('chevronLeft', 14), 'Libraries'), headerSlot,
    filterBar({ days, onDays: (v) => { days = v; saveDays(v); replaceQuery({ days }); dv.load(); } }), view, makeupSlot(ctx, id));
  dv.load();
}

export function itemGrid(items) {
  if (!items || !items.length) return h('div', { class: 'chart-empty chart-empty-sm' }, 'Nothing added yet.');
  return h('ul', { class: 'item-grid' }, items.map((it) => h('li', null, h('a', { class: 'item-card', href: `/items/${it.id}` },
    poster(it.image_item_id || it.id, it.name, { w: 300, cls: 'poster-grid' }),
    h('span', { class: 'item-card-name' }, it.name),
    h('span', { class: 'item-card-sub' }, [it.sub || it.year, it.date_created ? 'added ' + relTime(it.date_created) : null].filter(Boolean).join(' · '))))));
}
