// The all-time part of a profile: day streaks and how far someone is through each show.
// One segment per episode that exists as a file on the server; on your own profile a segment
// (or a whole season / show) can be marked as seen by hand.

import { h, icon, mount, num, pct, parseDay } from '../dom.js';
import { api, isAbort } from '../api.js';
import { card, segmented, poster, statTile, emptyState, errorState, inlineError, sk, setBusy } from '../components.js';

const PAGE = 6;
const dayLabel = (iso, withYear) => parseDay(iso).toLocaleDateString(undefined, { day: 'numeric', month: 'short', ...(withYear ? { year: 'numeric' } : {}) });

function streakTiles(s) {
  if (!s) return null;
  const l = s.longest;
  const c = s.current || {};
  return h('div', { class: 'tiles tiles-3' },
    statTile({ label: 'Longest day streak', value: l ? `${num(l.days)} ${l.days === 1 ? 'day' : 'days'}` : '–',
      hint: l ? (l.days > 1 ? `${dayLabel(l.from)} – ${dayLabel(l.to, true)}` : dayLabel(l.from, true)) : 'No plays yet' }),
    statTile({ label: 'Current streak', value: c.days ? `${num(c.days)} ${c.days === 1 ? 'day' : 'days'}` : '–',
      hint: !c.days ? 'Nothing played today or yesterday' : c.includes_today ? `Since ${dayLabel(c.since)}` : `Since ${dayLabel(c.since)} · play something today to keep it` }),
    statTile({ label: 'Days with a play', value: num(s.active_days || 0), hint: 'all time' }));
}

const SOURCE = { played: 'watched', jellyfin: 'marked played in Jellyfin', manual: 'marked by you' };
const epLabel = (seasonNo, e) => `S${String(seasonNo).padStart(2, '0')}E${String(e.episode_number ?? '?').padStart(2, '0')} · ${e.name}`;

function bar(episodes, { seasonNo, editable, onToggle, dense }) {
  return h('div', { class: ['ep-bar', (dense || episodes.length > 120) && 'ep-bar-dense'], role: editable ? 'group' : 'img',
    'aria-label': `${episodes.filter((e) => e.state === 'seen').length} of ${episodes.length} episodes seen` },
    episodes.map((e) => {
      const what = e.state === 'seen' ? `seen (${SOURCE[e.source] || 'seen'})` : e.state === 'started' ? 'started, not finished' : 'not seen';
      const title = `${epLabel(e.season ?? seasonNo, e)} — ${what}`;
      if (!editable) return h('span', { class: `ep ep-${e.state}`, title });
      const locked = e.state === 'seen' && e.source !== 'manual'; // a real play can't be un-watched here
      return h('button', { type: 'button', class: `ep ep-${e.state}`, title: locked ? title : `${title}. Click to ${e.state === 'seen' ? 'unmark' : 'mark as seen'}.`,
        'aria-label': title, 'aria-pressed': String(e.state === 'seen'), 'aria-disabled': locked ? 'true' : null,
        onClick: () => { if (!locked) onToggle([e.id], e.state !== 'seen'); } });
    }));
}

/**
 * Returns two elements fed by one request: `tiles` (the streaks, for the top of the profile) and
 * `shows` (the progress card, placed further down). Both cover all time, whatever range the rest
 * of the page shows; the same nodes are handed back on every re-render, so an opened show or a
 * chosen tab survives a change of range.
 */
export function profileAllTime({ userId, signal }) {
  const tiles = h('div', { class: 'profile-alltime' });
  const showsEl = h('div', { class: 'profile-shows' });
  let data = null;
  let filter = 'progress';
  let shown = PAGE;
  const open = new Set();
  const problem = h('div');

  async function load(first) {
    if (first) { mount(tiles, sk.tiles(3)); mount(showsEl, sk.cardRows(4)); }
    try { data = await api.get(`/users/${userId}/shows`, null, { signal }); paint(); }
    catch (e) { if (isAbort(e) || e.status === 401) return; if (first) { mount(tiles, ''); mount(showsEl, errorState(e, () => load(true))); } }
  }

  async function toggle(ids, seen, btn) {
    mount(problem, '');
    if (btn) setBusy(btn, true);
    try { await api.post('/me/seen', { item_ids: ids, seen }); await load(false); }
    catch (e) { if (btn) setBusy(btn, false); mount(problem, inlineError('seen-err', `Couldn’t save: ${e.message}`)); }
  }

  function row(s) {
    const all = s.seasons.flatMap((se) => se.episodes.map((e) => ({ ...e, season: se.season_number })));
    const isOpen = open.has(s.id);
    const done = s.seen >= s.total;
    const head = h('button', { type: 'button', class: 'show-head', 'aria-expanded': String(isOpen),
      onClick: () => { if (isOpen) open.delete(s.id); else open.add(s.id); paint(); } },
      h('span', { class: 'show-name' }, s.name, s.removed ? h('span', { class: 'chip' }, 'No longer in library') : null),
      h('span', { class: 'show-count mono' }, `${num(s.seen)}/${num(s.total)}`), h('span', { class: 'show-pct mono' }, pct(s.total ? s.seen / s.total : 0)),
      icon('chevronRight', 14, 'show-chev'));
    const unseen = (eps) => eps.filter((e) => e.state !== 'seen').map((e) => e.id);
    const manual = (eps) => eps.filter((e) => e.source === 'manual').map((e) => e.id);
    const markBtn = (eps, label) => {
      if (!data.editable) return null;
      const todo = unseen(eps);
      const undo = manual(eps);
      if (todo.length) { const b = h('button', { type: 'button', class: 'btn btn-ghost btn-sm' }, icon('check', 13), label); b.addEventListener('click', () => toggle(todo, true, b)); return b; }
      if (undo.length) { const b = h('button', { type: 'button', class: 'btn btn-ghost btn-sm' }, 'Undo my marks'); b.addEventListener('click', () => toggle(undo, false, b)); return b; }
      return null;
    };
    return h('li', { class: ['show-row', isOpen && 'is-open', done && 'is-done'] },
      s.removed ? h('span', { class: 'poster poster-sm poster-ph', 'aria-hidden': 'true' }, icon('tv', 16)) : h('a', { href: `/items/${s.id}`, tabindex: -1, 'aria-hidden': 'true' }, poster(s.image_item_id, s.name, { w: 160, cls: 'poster-sm' })),
      h('div', { class: 'show-main' }, head,
        isOpen ? null : bar(all, { editable: false }),
        isOpen ? h('div', { class: 'show-seasons' },
          s.seasons.map((se) => h('div', { class: 'season-row' },
            h('div', { class: 'season-head' }, h('span', { class: 'season-name' }, `Season ${se.season_number}`),
              h('span', { class: 'mono season-count' }, `${num(se.seen)}/${num(se.total)} (${pct(se.total ? se.seen / se.total : 0)})`), markBtn(se.episodes, 'Mark season as seen')),
            bar(se.episodes, { seasonNo: se.season_number, editable: data.editable, onToggle: (ids, seen) => toggle(ids, seen) }))),
          data.editable ? h('div', { class: 'season-foot' }, markBtn(all, 'Mark the whole show as seen'),
            h('span', { class: 'help' }, 'Click an episode to mark it. Marks stay in finstats and never change anything in Jellyfin.')) : null) : null));
  }

  function paint() {
    const shows = (data.shows || []).map((s) => ({ ...s, ratio: s.total ? s.seen / s.total : 0 }));
    const lists = {
      progress: shows.filter((s) => s.seen < s.total).sort((a, b) => b.ratio - a.ratio || (b.last_played_at || 0) - (a.last_played_at || 0)),
      finished: shows.filter((s) => s.total > 0 && s.seen >= s.total).sort((a, b) => (b.last_played_at || 0) - (a.last_played_at || 0)),
    };
    lists.all = [...lists.progress, ...lists.finished];
    const list = lists[filter];
    const body = !shows.length ? emptyState('No shows yet.', 'Episodes show up here once something from a series has been played.')
      : !list.length ? emptyState(filter === 'finished' ? 'No finished shows yet.' : 'Nothing in progress. Everything started has been finished.')
      : [h('ul', { class: 'show-list' }, list.slice(0, shown).map(row)),
        list.length > shown ? h('button', { type: 'button', class: 'btn btn-ghost show-more', onClick: () => { shown = list.length; paint(); } }, `Show all ${num(list.length)}`) : null];
    mount(tiles, streakTiles(data.streaks));
    mount(showsEl,
      card({ title: 'Shows', sub: 'Episodes seen, out of those on the server. All time.',
        actions: shows.length ? segmented({ label: 'Which shows', value: filter, onChange: (v) => { filter = v; shown = PAGE; paint(); },
          options: [{ value: 'progress', label: `In progress · ${lists.progress.length}` }, { value: 'finished', label: `Finished · ${lists.finished.length}` }, { value: 'all', label: 'All' }] }) : null,
        body: [body, problem] }));
  }

  load(true);
  return { tiles, shows: showsEl };
}
