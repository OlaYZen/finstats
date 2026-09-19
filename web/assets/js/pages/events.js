import { h, icon, debounce, num, dateTime, relTime } from '../dom.js';
import { api } from '../api.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, dataView, sk, pagination, emptyState } from '../components.js';

const PER_PAGE = 50;
// Status colors always ship with an icon + label.
const SEVERITY = {
  error: { cls: 'sev-critical', icon: 'alert', label: 'Error' }, critical: { cls: 'sev-critical', icon: 'alert', label: 'Critical' },
  fatal: { cls: 'sev-critical', icon: 'alert', label: 'Fatal' },
  warn: { cls: 'sev-warning', icon: 'alert', label: 'Warning' }, warning: { cls: 'sev-warning', icon: 'alert', label: 'Warning' },
  information: { cls: 'sev-info', icon: 'info', label: 'Info' }, info: { cls: 'sev-info', icon: 'info', label: 'Info' },
  debug: { cls: 'sev-info', icon: 'info', label: 'Debug' }, trace: { cls: 'sev-info', icon: 'info', label: 'Trace' },
};

export default function events(ctx) {
  ctx.title('Server log');
  const f = { q: ctx.query.get('q') || '', type: ctx.query.get('type') || '', page: Math.max(1, Number(ctx.query.get('page')) || 1) };
  const view = h('div');
  const summary = h('p', { class: 'result-count', 'aria-live': 'polite' });
  const typeSlot = h('span');

  function paintType() {
    typeSlot.replaceChildren(f.type ? h('span', { class: 'chip chip-removable' }, 'Type: ' + f.type,
      h('button', { type: 'button', class: 'chip-x', 'aria-label': 'Remove type filter', onClick: () => { f.type = ''; apply(); } }, icon('x', 12))) : '');
  }
  function apply(reset = true) {
    if (reset) f.page = 1;
    paintType();
    replaceQuery({ q: f.q, type: f.type, page: f.page > 1 ? f.page : '' });
    dv.load();
  }

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => sk.tableRows(5),
    fetch: () => api.get('/events', { ...f, per_page: PER_PAGE }, { signal: ctx.signal }),
    render: (data) => {
      summary.textContent = `${num(data.total)} ${data.total === 1 ? 'entry' : 'entries'}`;
      if (!data.rows.length) return emptyState(f.q || f.type ? 'No log entries match these filters.' : 'No log entries yet', f.q || f.type ? null : 'Entries arrive with the next “Sync server log” task.');
      return [h('div', { class: 'table-scroll' }, h('table', { class: 'table events' },
        h('thead', null, h('tr', null, h('th', null, 'When'), h('th', null, 'Level'), h('th', null, 'Event'), h('th', null, 'User'), h('th', null, 'Type'))),
        h('tbody', null, data.rows.map((e) => {
          const sev = SEVERITY[String(e.severity || '').toLowerCase()] || SEVERITY.info;
          return h('tr', null,
            h('td', { class: 'mono nowrap', title: dateTime(e.date) }, relTime(e.date)),
            h('td', null, h('span', { class: 'sev ' + sev.cls }, icon(sev.icon, 13), sev.label)),
            h('td', null, h('div', { class: 'event-name' }, e.name), e.overview ? h('div', { class: 'event-overview' }, e.overview) : null),
            h('td', null, e.user_id ? h('a', { href: `/users/${e.user_id}` }, e.user_name || 'User') : h('span', { class: 'muted' }, '–')),
            h('td', null, e.type ? h('button', { type: 'button', class: 'chip chip-btn mono', title: 'Show only this type', onClick: () => { f.type = e.type; apply(); } }, e.type) : null));
        })))),
        data.total > PER_PAGE ? pagination({ page: data.page || f.page, perPage: data.per_page || PER_PAGE, total: data.total, onPage: (p) => { f.page = p; apply(false); window.scrollTo({ top: 0 }); } }) : null];
    },
  });

  const search = h('input', { class: 'input input-search', type: 'search', placeholder: 'Search the log…', value: f.q, 'aria-label': 'Search the log', autocomplete: 'off' });
  const onSearch = debounce(() => { f.q = search.value.trim(); apply(); }, 250);
  search.addEventListener('input', onSearch);
  ctx.onCleanup(() => onSearch.cancel());

  paintType();
  ctx.root.append(pageHeader('Server log', 'Jellyfin’s own activity log: sign-ins, failed logins, playback, tasks'),
    h('div', { class: 'filters' }, h('div', { class: 'search-field' }, icon('search', 14), search), typeSlot),
    summary, card({ cls: 'card-flush', body: view }));
  dv.load();
}
