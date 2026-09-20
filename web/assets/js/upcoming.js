// What Sonarr and Radarr expect: shared by the Pipeline page, the dashboard row, the profile and the title page.

import { h, icon, mount, initials, episodeCode, parseDay } from './dom.js';
import { api, imgItem } from './api.js';

export const RELEASE_LABEL = { air: 'Airs', cinema: 'In cinemas', digital: 'Digital release', physical: 'Disc release' };
const FINALE_LABEL = { season: 'Season finale', series: 'Series finale', midseason: 'Mid-season finale' };

/** The same loader for the page, the cards and the prefetcher. */
export const loadUpcoming = ({ days = 14, userId = '', mine = false } = {}, signal) => api.get('/upcoming', { days, user_id: userId, mine: mine ? 'true' : '' }, { signal });

/** A title in the library has its poster there; one that is still to come only Sonarr or Radarr can show (through finstats). */
export function upcomingPoster(e, { w = 160, cls = '' } = {}) {
  const name = e.series_title || e.title;
  const box = h('span', { class: 'poster ' + cls, 'aria-hidden': 'true' });
  const fallback = () => mount(box, h('span', { class: 'poster-fallback' }, initials(name)));
  const p = e.poster || {};
  const src = p.item_id ? imgItem(p.item_id, w) : p.service_id ? `/api/img/arr/${p.service_id}/${p.media_id}?w=${w}` : null;
  if (!src) { fallback(); return box; }
  box.append(h('img', { src, alt: '', loading: 'lazy', decoding: 'async', onError: fallback }));
  return box;
}

// An air time is a time of day to the minute; the seconds `timeOfDay` shows belong to a play's timeline.
const hourMinute = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });
const timeOfDay = (ts) => hourMinute.format(new Date(ts * 1000));
const dayMs = 86400e3;
const weekday = new Intl.DateTimeFormat(undefined, { weekday: 'long' });
const dayMonth = new Intl.DateTimeFormat(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
/** "Today", "Tomorrow", "Friday", then "Fri 2 Oct". `day` is a local YYYY-MM-DD. */
export function dayName(day) {
  const d = parseDay(day), today = new Date(); today.setHours(0, 0, 0, 0);
  const diff = Math.round((d - today) / dayMs);
  if (diff === 0) return 'Today';
  if (diff === 1) return 'Tomorrow';
  if (diff > 1 && diff < 7) return weekday.format(d);
  return dayMonth.format(d);
}

export const entryName = (e) => e.series_title || e.title;
/** "S03E10 · Re-entry" for an episode, "Digital release" for a film. */
export function entryWhat(e) {
  if (e.kind !== 'episode') return RELEASE_LABEL[e.release] || 'Release';
  const code = episodeCode(e.season, e.episode);
  return [code, e.title && e.title !== 'TBA' ? e.title : null].filter(Boolean).join(' · ') || 'New episode';
}
export const entryHref = (e) => (e.item_id ? `/items/${e.item_id}` : null);

/** One line of an agenda. `people`: also say who follows it (only ever sent to those who may know). */
export function entryRow(e, { people = false } = {}) {
  const href = entryHref(e);
  const title = href ? h('a', { class: 'up-title', href }, entryName(e)) : h('span', { class: 'up-title' }, entryName(e));
  const names = people && e.follower_names && e.follower_names.length ? e.follower_names : null;
  return h('li', { class: 'up-row' },
    upcomingPoster(e, { w: 96, cls: 'poster-sm' }),
    h('div', { class: 'up-main' },
      h('div', { class: 'up-line' }, title, e.kind === 'movie' && e.year ? h('span', { class: 'muted' }, ` (${e.year})`) : null),
      h('div', { class: 'up-what' }, entryWhat(e)),
      h('div', { class: 'up-tags' },
        e.finale ? h('span', { class: 'chip' }, FINALE_LABEL[e.finale] || 'Finale') : null,
        e.has_file ? h('span', { class: 'sev sev-good' }, icon('check', 13), 'Already here') : null,
        !href ? h('span', { class: 'chip', title: 'Sonarr or Radarr is waiting for it; it is not in your Jellyfin library yet' }, 'New to the library') : null,
        e.you_follow ? h('span', { class: 'sev up-follow' }, icon('heart', 13), 'You watch this') : null,
        names ? h('span', { class: 'up-people', title: names.join(', ') }, icon('users', 13), names.length <= 3 ? names.join(', ') : `${names.slice(0, 2).join(', ')} +${names.length - 2}`) : null)),
    h('div', { class: 'up-when mono' }, e.at ? timeOfDay(e.at) : ''));
}

/** Entries grouped under their day. */
export function agenda(entries, opts) {
  const days = [];
  for (const e of entries) {
    if (!days.length || days[days.length - 1].day !== e.day) days.push({ day: e.day, list: [] });
    days[days.length - 1].list.push(e);
  }
  return h('div', { class: 'agenda' }, days.map((d) => h('section', { class: 'agenda-day' },
    h('h3', { class: 'agenda-head' }, dayName(d.day), h('span', { class: 'agenda-count mono' }, String(d.list.length))),
    h('ul', { class: 'up-list' }, d.list.map((e) => entryRow(e, opts))))));
}

/** A poster card for the dashboard row. */
export function entryCard(e) {
  const inner = [upcomingPoster(e, { w: 300, cls: 'poster-grid' }),
    h('span', { class: 'shelf-when' }, dayName(e.day), e.at ? ` · ${timeOfDay(e.at)}` : ''),
    h('span', { class: 'shelf-name' }, entryName(e)),
    h('span', { class: 'shelf-sub' }, entryWhat(e)),
    e.has_file ? h('span', { class: 'shelf-sub' }, 'Already here') : null];
  const href = entryHref(e);
  return href ? h('a', { class: 'shelf-card', href }, inner) : h('span', { class: 'shelf-card' }, inner);
}
