import { h, icon, debounce, num } from '../dom.js';
import { api } from '../api.js';
import { isAdmin, readDays, saveDays, rangeLong } from '../state.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, filterBar, dataView, sk, playsTable, pagination, segmented } from '../components.js';
import { openPlayModal } from '../playmodal.js';

const METHODS = [
  { value: '', label: 'All' }, { value: 'DirectPlay', label: 'Direct play' },
  { value: 'DirectStream', label: 'Direct stream' }, { value: 'Transcode', label: 'Transcode' },
];
const TYPES = [
  { value: '', label: 'All' }, { value: 'Movie', label: 'Movies' }, { value: 'Episode', label: 'Episodes' }, { value: 'Audio', label: 'Music' },
];
const PER_PAGE = 50;

export default function activity(ctx) {
  ctx.title('Activity');
  const q0 = ctx.query;
  const f = {
    days: readDays(q0),
    user_id: isAdmin() ? q0.get('user_id') || '' : '',
    method: q0.get('method') || '',
    type: q0.get('type') || '',
    q: q0.get('q') || '',
    item_id: q0.get('item_id') || '',
    series_id: q0.get('series_id') || '',
    page: Math.max(1, Number(q0.get('page')) || 1),
  };

  const view = h('div');
  const summary = h('p', { class: 'result-count', 'aria-live': 'polite' });
  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => sk.tableRows(5),
    fetch: () => api.get('/activity', { ...f, per_page: PER_PAGE }, { signal: ctx.signal }),
    render: (data) => {
      summary.textContent = `${num(data.total)} ${data.total === 1 ? 'play' : 'plays'} · ${rangeLong(f.days).toLowerCase()}`;
      return [
        playsTable(data.rows, { showUser: isAdmin(), onOpen: (p) => openPlayModal(p, { onDeleted: () => dv.load() }),
          empty: 'No plays match these filters. Try a longer range or clear the search.' }),
        data.total > PER_PAGE ? pagination({ page: data.page || f.page, perPage: data.per_page || PER_PAGE, total: data.total,
          onPage: (p) => { f.page = p; apply(false); window.scrollTo({ top: 0 }); } }) : null,
      ];
    },
  });

  function apply(resetPage = true) {
    if (resetPage) f.page = 1;
    replaceQuery({ ...f, page: f.page > 1 ? f.page : '' });
    dv.load();
  }

  const search = h('input', { class: 'input input-search', type: 'search', placeholder: 'Search titles…', value: f.q, 'aria-label': 'Search titles', autocomplete: 'off' });
  const onSearch = debounce(() => { f.q = search.value.trim(); apply(); }, 250);
  search.addEventListener('input', onSearch);
  ctx.onCleanup(() => onSearch.cancel());

  const scopeChip = f.item_id || f.series_id ? h('span', { class: 'chip chip-removable' }, f.series_id ? 'One series' : 'One title',
    h('button', { type: 'button', class: 'chip-x', 'aria-label': 'Remove title filter', onClick: (e) => { f.item_id = ''; f.series_id = ''; e.target.closest('.chip').remove(); apply(); } }, icon('x', 12))) : null;

  const filters = filterBar({ days: f.days, userId: f.user_id, signal: ctx.signal,
    onDays: (v) => { f.days = v; saveDays(v); apply(); },
    onUser: (v) => { f.user_id = v; apply(); },
    extra: [
      segmented({ label: 'Play method', options: METHODS, value: f.method, onChange: (v) => { f.method = v; apply(); } }),
      segmented({ label: 'Media type', options: TYPES, value: f.type, onChange: (v) => { f.type = v; apply(); } }),
      h('div', { class: 'search-field' }, icon('search', 14), search),
      scopeChip,
    ] });

  ctx.root.append(pageHeader('Activity', 'Every play finstats knows about, newest first'), filters, summary,
    card({ cls: 'card-flush', body: view }));
  dv.load();
}
