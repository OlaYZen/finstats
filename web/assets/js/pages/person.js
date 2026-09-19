// One actor or director: what they are in on this server, and how much of it has been watched.

import { h, icon, num, duration, durationExact, durEl, relEl, compact } from '../dom.js';
import { api } from '../api.js';
import { readDays, saveDays, can } from '../state.js';
import { replaceQuery } from '../router.js';
import { card, filterBar, dataView, sk, poster, avatar, chip, statTile, emptyState } from '../components.js';

const TYPE_LABEL = { Movie: 'Movie', Series: 'Series' };

export default function personPage(ctx) {
  const id = ctx.params.id;
  ctx.title('Person');
  let days = readDays(ctx.query);
  const view = h('div', { class: 'stack' });

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [h('div', { class: 'item-hero' }, h('span', { class: 'sk sk-poster-lg' }), h('div', { class: 'sk-row-lines' }, sk.line('40%', 28), sk.line('30%'))), sk.tiles(3), sk.cardBlock(320)],
    fetch: () => api.get(`/people/${id}`, { days }, { signal: ctx.signal }),
    render: (d) => {
      const p = d.person, t = d.totals || {};
      ctx.title(p.name);
      const titles = d.titles || [];
      const watched = titles.filter((x) => x.plays > 0), unwatched = titles.filter((x) => !x.plays);
      return [
        h('div', { class: 'item-hero' },
          poster(p.has_image ? p.id : null, p.name, { w: 300, cls: 'poster-lg' }),
          h('div', { class: 'item-hero-text' },
            h('h1', { class: 'page-title' }, p.name),
            h('div', { class: 'chips' }, p.is_actor ? chip('Actor') : null, p.is_director ? chip('Director') : null,
              chip(`${num(p.titles)} ${p.titles === 1 ? 'title' : 'titles'} in the library`)))),
        h('div', { class: 'tiles tiles-3' },
          statTile({ label: 'Watch time', value: duration(t.watch_s), title: durationExact(t.watch_s), hint: 'across everything they are in' }),
          statTile({ label: 'Plays', value: compact(t.plays), title: num(t.plays), hint: t.last_played_at ? ['last ', relEl(t.last_played_at, '')] : 'Never played' }),
          statTile({ label: 'Titles watched', value: `${num(t.titles_watched)} of ${num(p.titles)}`, hint: ' ' })),
        card({ title: 'Watched', sub: 'In this range, most watched first', body: watched.length ? titleGrid(watched, true) : emptyState('Nothing with them was played in the selected range.') }),
        unwatched.length ? card({ title: 'Also in the library', sub: 'Not played in this range', body: titleGrid(unwatched, false) }) : null,
        can('see_everyone') && d.watchers && d.watchers.length ? card({ title: 'Watched by', cls: 'card-flush', body: watchers(d.watchers) }) : null,
      ];
    },
  });

  ctx.root.append(filterBar({ days, onDays: (v) => { days = v; saveDays(v); replaceQuery({ days }); dv.load(); } }), view);
  dv.load();
}

function titleGrid(rows, withTime) {
  return h('ul', { class: 'item-grid' }, rows.map((x) => {
    const credit = String(x.kinds || '').includes('Director') ? (String(x.kinds).includes('Actor') ? (x.role ? `Director · as ${x.role}` : 'Director · Actor') : 'Director') : x.role ? `as ${x.role}` : null;
    const inner = [
      poster(x.id, x.name, { w: 300, cls: 'poster-grid' }),
      h('span', { class: 'item-card-name' }, x.name),
      h('span', { class: 'item-card-sub' }, [TYPE_LABEL[x.type] || x.type, x.year, x.removed ? 'no longer in library' : null].filter(Boolean).join(' · ')),
      credit ? h('span', { class: 'item-card-sub' }, credit) : null,
      withTime ? h('span', { class: 'item-card-sub mono', title: durationExact(x.watch_s) }, `${duration(x.watch_s)} · ${num(x.plays)} ${x.plays === 1 ? 'play' : 'plays'}`) : null,
    ];
    return h('li', null, h('a', { class: 'item-card', href: `/items/${x.id}` }, inner));
  }));
}

function watchers(rows) {
  return h('div', { class: 'table-scroll' }, h('table', { class: 'table' },
    h('thead', null, h('tr', null, h('th', null, 'User'), h('th', { class: 'r' }, 'Plays'), h('th', { class: 'r' }, 'Watch time'), h('th', null, 'Last played'))),
    h('tbody', null, rows.map((w) => h('tr', null,
      h('td', null, h('a', { class: 'user-cell', href: `/users/${w.user_id}` }, avatar(w.user_id, w.user_name, { size: 22 }), h('span', null, w.user_name))),
      h('td', { class: 'mono r' }, num(w.plays)), h('td', { class: 'r' }, durEl(w.watch_s)), h('td', null, relEl(w.last_played_at)))))));
}
