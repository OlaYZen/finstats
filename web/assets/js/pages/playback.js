import { h, humanize, store } from '../dom.js';
import { api, soft } from '../api.js';
import { readDays, saveDays, can } from '../state.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, chartCard, filterBar, dataView, sk, segmented } from '../components.js';
import { bucketList, methodsBar, simpleColumns, simpleColumnsTable, clientMethods, methodLegend } from '../charts.js';
import { num, duration, pct, dayLabel, dayLabelLong, dateTime } from '../dom.js';
import { chartTable } from '../tables.js';

const upper = (x) => (x && x.length <= 5 ? x.toUpperCase() : x);
const chLabel = (x) => ({ 1: 'Mono', 2: 'Stereo', 6: '5.1', 8: '7.1' }[x] || (/^\d+$/.test(String(x)) ? `${x} channels` : x));

export default function playback(ctx) {
  ctx.title('Playback');
  let days = readDays(ctx.query);
  let userId = can('see_everyone') ? ctx.query.get('user_id') || '' : '';
  let metric = store.get('finstats.methodMetric', 'plays') === 'watch_s' ? 'watch_s' : 'plays';
  const view = h('div', { class: 'stack' });

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.cardBlock(80), h('div', { class: 'grid-3' }, sk.cardRows(5), sk.cardRows(5), sk.cardRows(5))],
    fetch: async () => {
      const f = { days, user_id: userId }, o = { signal: ctx.signal };
      const [d, ins] = await Promise.all([api.get('/stats/playback', f, o), soft(api.get('/stats/insights', f, o))]);
      return { d, ins };
    },
    render: ({ d, ins }) => {
      const methodsCard = chartCard({
        title: 'Play methods', sub: 'Direct play streams the file untouched; transcoding costs server CPU or GPU',
        controls: segmented({ label: 'Measure', size: 'seg-sm', value: metric, options: [{ value: 'plays', label: 'Plays' }, { value: 'watch_s', label: 'Watch time' }],
          onChange: (v) => { metric = v; store.set('finstats.methodMetric', v); methodsCard.rerender(); } }),
        chart: () => methodsBar(d.methods, metric),
        table: () => methodsTable(d.methods),
      });
      const b = (title, sub, rows, labelFn, empty) => card({ title, sub, body: bucketList(rows, { labelFn, empty }) });
      return [
        methodsCard,
        insightCards(ins),
        h('div', { class: 'grid-3' },
          b('Why streams transcode', 'Reasons reported by Jellyfin', d.transcode_reasons, humanize, 'Nothing was transcoded in this range.'),
          b('Transcoding hardware', 'Acceleration used', d.hw_accel, (x) => (x && x !== 'None' ? upper(x) : 'Software'), 'Nothing was transcoded in this range.'),
          b('Clients', 'Apps people play with', d.clients)),
        h('div', { class: 'grid-3' },
          b('Video codecs', 'Source files', d.video_codecs, upper),
          b('Resolutions', 'Source files', d.resolutions),
          b('Dynamic range', 'Source files', d.video_ranges)),
        h('div', { class: 'grid-3' },
          b('Audio codecs', 'Selected audio track', d.audio_codecs, upper),
          b('Audio channels', 'Selected audio track', d.audio_channels, chLabel),
          b('Containers', 'Source files', d.containers, upper)),
        h('div', { class: 'grid-3' },
          b('Subtitles', 'Selected subtitle language', d.subtitles, (x) => (x === 'None' ? 'Off' : x))),
      ];
    },
  });

  const sync = () => replaceQuery({ days, user_id: userId });
  ctx.root.append(pageHeader('Playback', 'How media reaches people: play methods, codecs and clients'),
    filterBar({ days, userId, signal: ctx.signal, onDays: (v) => { days = v; saveDays(v); sync(); dv.load(); }, onUser: (v) => { userId = v; sync(); dv.load(); } }),
    view);
  dv.load();
}

/** Cards fed by /api/stats/insights. Every one of them is optional. */
function insightCards(ins) {
  if (!ins) return null;
  const c = ins.concurrency || {};
  const week = c.bucket === 'week';
  const series = (c.series || []).map((x) => ({ label: dayLabel(x.date), title: (week ? 'Week of ' : '') + dayLabelLong(x.date), value: x.peak }));
  const beh = ins.behaviour || {};
  const net = Array.isArray(ins.network) ? ins.network : [];
  const one = (x) => (x == null ? '–' : Number(x).toFixed(1));

  const concurrency = chartCard({
    title: 'Concurrent streams',
    sub: c.peak ? [`Most at once per ${week ? 'week' : 'day'} · peak of ${num(c.peak)}`, c.peak_at ? ` on ${dateTime(c.peak_at)}` : '', c.peak_transcodes ? ` · up to ${num(c.peak_transcodes)} transcoding at once` : ''].join('') : `Most at once per ${week ? 'week' : 'day'}`,
    chart: () => simpleColumns({ rows: series, unit: ['stream', 'streams'], ariaLabel: 'Peak concurrent streams', empty: 'No plays in this range.' }),
    table: () => simpleColumnsTable({ rows: series, head: [week ? 'Week of' : 'Date', 'Peak streams'] }),
  });
  const clients = card({ title: 'Which clients transcode', sub: 'Plays per client, split by play method', actions: methodLegend(), cls: 'card-legend', body: clientMethods(ins.client_methods) });
  const completion = card({ title: 'How far people get', sub: 'Movies and episodes, by where playback stopped',
    body: bucketList(ins.completion, { watch: false, empty: 'No movie or episode plays in this range.' }) });
  const network = can('see_network') && net.length ? card({ title: 'Network', sub: 'Where plays came from', body: bucketList(net) }) : null;
  const behaviour = beh.plays_measured > 0 ? card({ title: 'Viewing behaviour', sub: `Based on ${num(beh.plays_measured)} live ${beh.plays_measured === 1 ? 'play' : 'plays'}`,
    body: h('dl', { class: 'kpis' },
      h('div', null, h('dt', null, 'Pauses per play'), h('dd', { class: 'mono' }, one(beh.avg_pauses))),
      h('div', null, h('dt', null, 'Skips per play'), h('dd', { class: 'mono' }, one(beh.avg_seeks))),
      h('div', null, h('dt', null, 'Picked up mid-way'), h('dd', { class: 'mono' }, beh.resumed_share == null ? '–' : pct(beh.resumed_share)))) }) : null;
  return [concurrency, h('div', { class: 'grid-2' }, clients, completion), network || behaviour ? h('div', { class: 'grid-2' }, network, behaviour) : null];
}

function methodsTable(methods) {
  const rows = methods || [];
  const total = rows.reduce((a, m) => a + (m.plays || 0), 0);
  return chartTable(h('table', { class: 'table' },
    h('thead', null, h('tr', null, h('th', null, 'Method'), h('th', { class: 'r' }, 'Plays'), h('th', { class: 'r' }, 'Share'), h('th', { class: 'r' }, 'Watch time'))),
    h('tbody', null, rows.map((m) => h('tr', null, h('td', null, humanize(m.name)), h('td', { class: 'mono r' }, num(m.plays)),
      h('td', { class: 'mono r' }, total ? pct(m.plays / total, 1) : '–'), h('td', { class: 'mono r' }, duration(m.watch_s)))))));
}
