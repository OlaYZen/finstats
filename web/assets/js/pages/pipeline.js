// /pipeline: what is coming in. One page, a tab per question: what did people ask for, what airs when,
// what is arriving right now. A tab exists only when a connected service can answer it and the viewer may see it.

import { h, icon, num, store, debounce, duration, relTime, dateTime, pct, bytes, dayLabel, dayLabelLong } from '../dom.js';
import { state, can } from '../state.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, chartCard, dataView, sk, emptyState, segmented, userCombobox, statTile, pagination, avatar } from '../components.js';
import { dataTable } from '../tables.js';
import { simpleColumns, simpleColumnsTable, libBucketList } from '../charts.js';
import { loadUpcoming, agenda, upcomingPoster } from '../upcoming.js';
import { loadDownloads, downloadsList, nothingDownloading } from '../downloads.js';
import { api } from '../api.js';

const TABS = [
  { key: 'requests', label: 'Requests', feature: 'requests', sub: 'Who asked for what, how long it took, and whether it was ever watched' },
  { key: 'upcoming', label: 'Upcoming', feature: 'upcoming', sub: 'What Sonarr and Radarr expect, and who is waiting for it' },
  { key: 'downloads', label: 'Downloads', feature: 'downloads', perm: 'see_downloads', sub: 'What is arriving right now' },
];
const features = () => (state.user && state.user.features) || {};
export const pipelineTabs = () => TABS.filter((t) => features()[t.feature] && (!t.perm || can(t.perm)));
// In the menu as soon as a service can answer something — and always for an administrator, who is the only
// person who can connect one. A page that can only be reached by typing its address is a page nobody finds.
export const hasPipeline = () => pipelineTabs().length > 0 || !!(state.user && state.user.is_admin);

const SPANS = [{ value: 7, label: '7 days' }, { value: 14, label: '14 days' }, { value: 30, label: '30 days' }, { value: 90, label: '90 days' }];
const upcomingScope = (query) => ({
  days: SPANS.some((s) => s.value === Number(query.get('days'))) ? Number(query.get('days')) : Number(store.get('finstats.upcomingDays', 14)) || 14,
  userId: can('see_everyone') ? query.get('user_id') || '' : '',
  mine: query.get('mine') === '1',
});
const tabOf = (query) => (pipelineTabs().find((t) => t.key === query.get('tab')) || pipelineTabs()[0] || {}).key;
// The prefetcher asks for exactly what the page would. (Never anything live: see the Downloads tab.)
export const prefetchPipeline = ({ query, signal }) => {
  const tab = tabOf(query);
  if (tab === 'upcoming') return [() => loadUpcoming(upcomingScope(query), signal)];
  if (tab === 'requests') return [() => loadRequests(requestScope(query), signal)];
  return [];   // the downloads tab is live: asking for it in advance would keep three services busy for a page nobody opened
};

function tabBar(current) {
  const tabs = pipelineTabs();
  if (tabs.length < 2) return null;
  return h('nav', { class: 'seg entity-tabs', 'aria-label': 'Pipeline sections' }, tabs.map((t) =>
    h('a', { class: 'seg-btn', href: `/pipeline?tab=${t.key}`, 'aria-current': t.key === current ? 'page' : null }, t.label)));
}

// ---------------------------------------------------------------- requests

const PER_PAGE = 25;
const monthShort = new Intl.DateTimeFormat(undefined, { month: 'short' });
const monthLong = new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' });
// A colour always comes with an icon and a word.
const STATE = {
  pending: { cls: 'sev-warning', icon: 'clock', label: 'Waiting for approval' },
  approved: { cls: 'sev-info', icon: 'check', label: 'Approved' },
  processing: { cls: 'sev-info', icon: 'download', label: 'On its way' },
  partial: { cls: 'sev-info', icon: 'download', label: 'Partly here' },
  available: { cls: 'sev-good', icon: 'check', label: 'Here' },
  declined: { cls: 'sev-critical', icon: 'x', label: 'Declined' },
  failed: { cls: 'sev-critical', icon: 'alert', label: 'Failed' },
  removed: { cls: 'sev-info', icon: 'minus', label: 'Gone from Seerr' },
};
const STATUSES = [{ value: '', label: 'All' }, { value: 'open', label: 'Open' }, { value: 'arrived', label: 'Here' }, { value: 'declined', label: 'Refused' }];

const requestScope = (query) => ({
  status: STATUSES.some((s) => s.value === query.get('status')) ? query.get('status') : '',
  userId: can('see_everyone') ? query.get('user_id') || '' : '',
  q: query.get('q') || '',
  sort: query.get('sort') || '', dir: query.get('dir') || '',
  page: Math.max(1, Number(query.get('page')) || 1),
});
const loadRequests = (f, signal) => {
  const o = { signal };
  return Promise.all([
    api.get('/requests', { status: f.status, user_id: f.userId, q: f.q, sort: f.sort, dir: f.dir, page: f.page, per_page: PER_PAGE }, o),
    api.get('/requests/summary', { user_id: f.userId }, o),
  ]).then(([list, summary]) => ({ list, summary }));
};

/** What a season request is about: "Season 2", "Seasons 1–3", "Every season". */
function seasonsText(r) {
  const s = Array.isArray(r.seasons) ? r.seasons : [];
  if (r.media_type !== 'tv') return null;
  if (!s.length) return 'Every season';
  if (s.length === 1) return `Season ${s[0]}`;
  const run = s.every((n, i) => i === 0 || n === s[i - 1] + 1);
  return run ? `Seasons ${s[0]}–${s[s.length - 1]}` : `${s.length} seasons`;
}

function titleCell(r) {
  const name = r.title || 'Unknown title';
  const inner = [upcomingPoster({ poster: r.poster, title: name, series_title: null }, { w: 96, cls: 'poster-sm' }),
    h('span', { class: 'req-title-text' },
      h('span', { class: 'req-title-name' }, name, r.year ? h('span', { class: 'muted' }, ` (${r.year})`) : null),
      h('span', { class: 'req-title-sub' }, [seasonsText(r), r.is_4k ? '4K' : null].filter(Boolean).join(' · ') || (r.media_type === 'movie' ? 'Film' : 'Series')))];
  return r.item_id ? h('a', { class: 'req-title', href: `/items/${r.item_id}` }, inner) : h('span', { class: 'req-title' }, inner);
}

function requestsTab(ctx, root) {
  const f = requestScope(ctx.query);
  const view = h('div', { class: 'stack' });
  const apply = (resetPage = true) => {
    if (resetPage) f.page = 1;
    replaceQuery({ tab: 'requests', status: f.status, user_id: f.userId, q: f.q, sort: f.sort, dir: f.dir, page: f.page > 1 ? f.page : '' });
    dv.load();
  };

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.tiles(4), sk.cardBlock(320)],
    fetch: () => loadRequests(f, ctx.signal),
    render: ({ list, summary }) => {
      const t = summary.totals || {};
      const everyone = can('see_everyone');
      const watched = everyone && t.watched_by_anyone != null ? t.watched_by_anyone : t.watched;
      const table = list.rows.length ? dataTable(h('table', { class: 'table requests' },
        h('thead', null, h('tr', null,
          h('th', { 'data-key': 'title' }, 'Title'),
          everyone ? h('th', { 'data-key': 'user' }, 'Asked by') : null,
          h('th', { 'data-key': 'when', 'data-first': 'desc' }, 'Asked'),
          h('th', { 'data-key': 'state' }, 'State'),
          h('th', { 'data-key': 'arrived', 'data-first': 'asc' }, 'Took'),
          h('th', { 'data-key': 'watched' }, 'Watched'))),
        h('tbody', null, list.rows.map((r) => {
          const st = STATE[r.state] || STATE.approved;
          return h('tr', null,
            h('td', null, titleCell(r)),
            everyone ? h('td', null, r.user_id ? h('a', { class: 'user-cell', href: `/users/${r.user_id}` }, avatar(r.user_id, r.user_name, { size: 22, hasImage: r.has_image }), r.user_name || 'Unknown')
              : h('span', { class: 'user-cell muted', title: 'Seerr does not say which Jellyfin user this is' }, r.user_name || 'Unknown')) : null,
            h('td', { class: 'mono nowrap', title: dateTime(r.requested_at) }, relTime(r.requested_at)),
            h('td', null, h('span', { class: 'sev ' + st.cls }, icon(st.icon, 13), st.label)),
            h('td', { class: 'mono nowrap', 'data-sort': r.arrived_after_s == null ? '' : r.arrived_after_s }, r.arrived_after_s == null ? h('span', { class: 'muted' }, '–') : duration(r.arrived_after_s)),
            h('td', null, r.watched ? h('span', { class: 'sev sev-good' }, icon('check', 13), everyone ? 'By them' : 'Yes')
              : r.watched_by_anyone ? h('span', { class: 'sev sev-info' }, icon('users', 13), 'By someone else')
              : r.state === 'available' ? h('span', { class: 'muted' }, 'Not yet') : h('span', { class: 'muted' }, '–')));
        }))), { server: { key: f.sort, dir: f.dir, onSort: (key, dir) => { f.sort = key; f.dir = dir; apply(); } } })
        : emptyState(f.q || f.status || f.userId ? 'No requests match these filters' : 'No requests yet', 'Requests appear here minutes after somebody asks for something in Seerr.');

      const trend = (summary.trend || []).filter((m) => m.median_s != null).map((m) => {
        const d = new Date(`${m.month}-02T00:00:00`);
        return { label: monthShort.format(d), title: `${monthLong.format(d)} · ${num(m.arrived)} ${m.arrived === 1 ? 'title' : 'titles'}`, value: Math.round((m.median_s / 3600) * 10) / 10 };
      });
      const never = summary.never_played || [];
      return [
        h('div', { class: 'tiles' },
          statTile({ label: 'Titles asked for', value: num(t.titles), hint: t.requests !== t.titles ? `${num(t.requests)} requests in all` : ' ' }),
          statTile({ label: 'Still waiting', value: num(t.open), hint: t.open ? 'Not here yet' : 'Nothing outstanding' }),
          statTile({ label: 'Typical wait', value: summary.median_arrive_s == null ? '–' : duration(summary.median_arrive_s), hint: 'Median, asked to available' }),
          statTile({ label: 'Watched after arriving', value: t.arrived ? pct(watched / t.arrived) : '–', hint: `${num(watched)} of ${num(t.arrived)} that arrived` })),
        trend.length > 1 ? chartCard({ title: 'How long a request takes', sub: 'Median hours from asking to available, by the month it was asked in',
          chart: () => simpleColumns({ rows: trend, unit: ['hour', 'hours'], ariaLabel: 'Median hours to arrive per month' }),
          table: () => simpleColumnsTable({ rows: trend, head: ['Month', 'Hours to arrive'] }) }) : null,
        card({ cls: 'card-flush', body: [table, list.total > PER_PAGE ? pagination({ page: list.page, perPage: list.per_page, total: list.total, onPage: (p) => { f.page = p; apply(false); window.scrollTo({ top: 0 }); } }) : null] }),
        never.length ? card({ title: 'Arrived, never played', sub: 'Here for more than two weeks and nobody has watched it', cls: 'card-flush',
          body: dataTable(h('table', { class: 'table' },
            h('thead', null, h('tr', null, h('th', null, 'Title'), everyone ? h('th', null, 'Asked by') : null, h('th', { 'data-first': 'asc' }, 'Here since'))),
            h('tbody', null, never.map((r) => h('tr', null,
              h('td', null, titleCell(r)),
              everyone ? h('td', null, r.user_name || 'Unknown') : null,
              h('td', { class: 'mono nowrap', 'data-sort': r.available_at, title: dateTime(r.available_at) }, relTime(r.available_at))))))) }) : null,
        summary.people && summary.people.length > 1 ? card({ title: 'Who asks for what', sub: 'Titles requested, and how many were watched afterwards', cls: 'card-flush',
          body: dataTable(h('table', { class: 'table' },
            h('thead', null, h('tr', null, h('th', null, 'Person'), h('th', { class: 'r', 'data-first': 'desc' }, 'Asked for'), h('th', { class: 'r' }, 'Arrived'), h('th', { class: 'r' }, 'Watched'))),
            h('tbody', null, summary.people.map((p) => h('tr', null,
              h('td', null, p.user_id ? h('a', { class: 'user-cell', href: `/users/${p.user_id}` }, avatar(p.user_id, p.user_name, { size: 22 }), p.user_name) : h('span', { class: 'muted' }, p.user_name)),
              h('td', { class: 'mono r' }, num(p.requests)), h('td', { class: 'mono r' }, num(p.arrived)),
              h('td', { class: 'mono r' }, p.arrived ? `${num(p.watched)} · ${pct(p.watched / p.arrived)}` : '–')))))) }) : null,
      ];
    },
  });

  const search = h('input', { class: 'input input-search', type: 'search', placeholder: 'Search titles…', value: f.q, 'aria-label': 'Search requests', autocomplete: 'off' });
  const onSearch = debounce(() => { f.q = search.value.trim(); apply(); }, 250);
  search.addEventListener('input', onSearch);
  ctx.onCleanup(() => onSearch.cancel());
  root.append(
    h('div', { class: 'filters', role: 'group', 'aria-label': 'Filters' },
      segmented({ label: 'Which requests', value: f.status, options: STATUSES, onChange: (v) => { f.status = v; apply(); } }),
      can('see_everyone') ? userCombobox({ value: f.userId, signal: ctx.signal, onChange: (u) => { f.userId = u; apply(); } }) : null,
      h('div', { class: 'search-field' }, icon('search', 14), search)),
    view);
  dv.load();
}

function upcomingTab(ctx, root) {
  const f = upcomingScope(ctx.query);
  const view = h('div', { class: 'stack' });
  const summary = h('p', { class: 'result-count', 'aria-live': 'polite' });
  const apply = () => { replaceQuery({ tab: 'upcoming', days: f.days, user_id: f.userId, mine: f.mine ? '1' : '' }); dv.load(); };
  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.cardRows(4), sk.cardRows(3)],
    fetch: () => loadUpcoming(f, ctx.signal),
    render: (d) => {
      const list = d.entries || [];
      const episodes = list.filter((e) => e.kind === 'episode').length;
      summary.textContent = list.length ? `${num(episodes)} ${episodes === 1 ? 'episode' : 'episodes'} · ${num(list.length - episodes)} film ${list.length - episodes === 1 ? 'release' : 'releases'}` : '';
      if (!list.length) {
        return emptyState(f.mine ? 'Nothing coming up for the shows watched here' : 'Nothing on the calendar',
          f.mine ? 'A show counts as followed when an episode of it was played in the last four months.' : 'Sonarr and Radarr expect nothing in this period. Only monitored titles count.');
      }
      return card({ cls: 'card-agenda', body: agenda(list, { people: can('see_everyone') }) });
    },
  });
  const mineBtn = segmented({ label: 'Which titles', size: 'seg-sm', value: f.mine ? 'mine' : 'all',
    options: [{ value: 'all', label: 'Everything' }, { value: 'mine', label: f.userId ? 'Only what they watch' : 'Only what I watch' }],
    onChange: (v) => { f.mine = v === 'mine'; apply(); } });
  root.append(
    h('div', { class: 'filters', role: 'group', 'aria-label': 'Filters' },
      segmented({ label: 'How far ahead', value: f.days, options: SPANS, onChange: (v) => { f.days = v; store.set('finstats.upcomingDays', String(v)); apply(); } }),
      can('see_everyone') ? userCombobox({ value: f.userId, signal: ctx.signal, onChange: (u) => { f.userId = u; apply(); } }) : null,
      mineBtn),
    summary, view);
  dv.load();
}

const HISTORY_SPANS = [{ value: 7, label: '7 days' }, { value: 30, label: '30 days' }, { value: 90, label: '90 days' }, { value: 365, label: 'A year' }];

/** What came in over time: the same data as the live list, only after the fact. */
function historySection(ctx) {
  let days = Number(store.get('finstats.grabDays', 30)) || 30;
  const view = h('div', { class: 'stack' });
  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => sk.cardBlock(220),
    fetch: () => api.get('/downloads/history', { days }, { signal: ctx.signal }),
    render: (d) => {
      const t = d.totals || {};
      if (!t.imported && !t.failed && !t.grabbed) return null;
      const daily = (d.daily || []).map((x) => ({ label: dayLabel(x.day), title: dayLabelLong(x.day), value: Math.round((x.size_bytes / 1e9) * 10) / 10 }));
      const bucket = (title, sub, rows, unit) => card({ title, sub, body: libBucketList(rows, { unit }) });
      return [
        h('div', { class: 'tiles' },
          statTile({ label: 'Arrived', value: num(t.imported), hint: `in the last ${days === 365 ? 'year' : days + ' days'}` }),
          statTile({ label: 'Downloaded', value: bytes(t.size_bytes), hint: t.imported ? `${bytes(t.size_bytes / t.imported)} on average` : ' ' }),
          statTile({ label: 'Failed', value: num(t.failed), hint: t.grabbed ? `of ${num(t.grabbed)} grabbed` : ' ' })),
        chartCard({ title: 'Imported per day', sub: 'Gigabytes that finished downloading',
          chart: () => simpleColumns({ rows: daily, unit: ['GB', 'GB'], ariaLabel: 'Gigabytes imported per day' }),
          table: () => simpleColumnsTable({ rows: daily, head: ['Day', 'GB'] }) }),
        h('div', { class: 'grid-3' },
          bucket('Indexers', 'Where it came from', d.indexers, 'Files'),
          bucket('Quality', 'As Sonarr and Radarr sorted it', d.quality, 'Files'),
          bucket('Clients', 'What fetched it', d.clients, 'Files')),
        (d.failures || []).length ? card({ title: 'Failed downloads', sub: 'Grabbed and then given up on', cls: 'card-flush',
          body: dataTable(h('table', { class: 'table' },
            h('thead', null, h('tr', null, h('th', { 'data-first': 'desc' }, 'When'), h('th', null, 'Title'), h('th', null, 'Indexer'))),
            h('tbody', null, d.failures.map((f) => h('tr', null,
              h('td', { class: 'mono nowrap', 'data-sort': f.at, title: dateTime(f.at) }, relTime(f.at)),
              h('td', null, h('span', { class: 'dl-release mono' }, f.source || f.title || 'Unknown')),
              h('td', null, f.indexer || h('span', { class: 'muted' }, '–'))))))) }) : null,
      ];
    },
  });
  const el = h('section', { class: 'dl-history' },
    h('div', { class: 'filters' }, h('h2', { class: 'section-title' }, 'What came in'),
      segmented({ label: 'How far back', size: 'seg-sm', value: days, options: HISTORY_SPANS, onChange: (v) => { days = v; store.set('finstats.grabDays', String(v)); dv.load(); } })),
    view);
  dv.load();
  return el;
}

function downloadsTab(ctx, root) {
  const view = h('div');
  let failed = null;
  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => sk.cardBlock(240),
    fetch: () => loadDownloads(ctx.signal),
    render: (d) => {
      failed = null;
      if (nothingDownloading(d) && !(d.problems || []).length) {
        return emptyState('Nothing is downloading', d.sources ? 'Sonarr, Radarr and your torrent client have nothing on the go.' : 'Connect a torrent client, or Sonarr and Radarr, to see what is arriving.');
      }
      return card({ cls: 'card-flush dl-card', body: h('div', { class: 'dl-wrap' }, downloadsList(d)) });
    },
  });
  root.append(view, historySection(ctx));
  dv.load();
  // A live list: it is polled while this page is open, and only then.
  ctx.every(async () => {
    try { await dv.load(); failed = null; } catch (e) { if (!failed) failed = e; }
  }, 5000, { visibleOnly: true });
}

export default function pipelinePage(ctx) {
  ctx.title('Pipeline');
  const tabs = pipelineTabs();
  if (!tabs.length) {
    ctx.root.append(pageHeader('Pipeline', 'What is requested, coming and downloading'),
      emptyState('Nothing is connected yet', 'Connect Sonarr, Radarr, Seerr or a torrent client, and this page shows what people asked for, what airs when and what is arriving.',
        state.user && state.user.is_admin ? h('a', { class: 'btn btn-primary', href: '/settings#connections' }, icon('plus', 14), 'Add a connection') : null));
    return;
  }
  const current = tabOf(ctx.query);
  const tab = tabs.find((t) => t.key === current);
  ctx.title(`${tab.label} · Pipeline`);
  // Native append() prints a missing node as the text "null": with one tab there is no tab bar.
  ctx.root.append(...[pageHeader('Pipeline', tab.sub), tabBar(current)].filter(Boolean));
  if (current === 'upcoming') upcomingTab(ctx, ctx.root);
  else if (current === 'requests') requestsTab(ctx, ctx.root);
  else if (current === 'downloads') downloadsTab(ctx, ctx.root);
}
