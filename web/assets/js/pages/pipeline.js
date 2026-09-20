// /pipeline: what is coming in. One page, a tab per question: what did people ask for, what airs when,
// what is arriving right now. A tab exists only when a connected service can answer it and the viewer may see it.

import { h, icon, num, store } from '../dom.js';
import { state, can } from '../state.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, dataView, sk, emptyState, segmented, userCombobox } from '../components.js';
import { loadUpcoming, agenda } from '../upcoming.js';

const TABS = [
  { key: 'upcoming', label: 'Upcoming', feature: 'upcoming', sub: 'What Sonarr and Radarr expect, and who is waiting for it' },
];
const features = () => (state.user && state.user.features) || {};
export const pipelineTabs = () => TABS.filter((t) => features()[t.feature]);
export const hasPipeline = () => pipelineTabs().length > 0;

const SPANS = [{ value: 7, label: '7 days' }, { value: 14, label: '14 days' }, { value: 30, label: '30 days' }, { value: 90, label: '90 days' }];
const upcomingScope = (query) => ({
  days: SPANS.some((s) => s.value === Number(query.get('days'))) ? Number(query.get('days')) : Number(store.get('finstats.upcomingDays', 14)) || 14,
  userId: can('see_everyone') ? query.get('user_id') || '' : '',
  mine: query.get('mine') === '1',
});
const tabOf = (query) => (pipelineTabs().find((t) => t.key === query.get('tab')) || pipelineTabs()[0] || {}).key;
// The prefetcher asks for exactly what the page would. (Never anything live: see the Downloads tab.)
export const prefetchPipeline = ({ query, signal }) => (tabOf(query) === 'upcoming' ? [() => loadUpcoming(upcomingScope(query), signal)] : []);

function tabBar(current) {
  const tabs = pipelineTabs();
  if (tabs.length < 2) return null;
  return h('nav', { class: 'seg entity-tabs', 'aria-label': 'Pipeline sections' }, tabs.map((t) =>
    h('a', { class: 'seg-btn', href: `/pipeline?tab=${t.key}`, 'aria-current': t.key === current ? 'page' : null }, t.label)));
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
}
