import { h, humanize, store } from '../dom.js';
import { api } from '../api.js';
import { isAdmin, readDays, saveDays } from '../state.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, chartCard, filterBar, dataView, sk, segmented } from '../components.js';
import { bucketList, methodsBar } from '../charts.js';
import { num, duration, pct } from '../dom.js';

const upper = (x) => (x && x.length <= 5 ? x.toUpperCase() : x);
const chLabel = (x) => ({ 1: 'Mono', 2: 'Stereo', 6: '5.1', 8: '7.1' }[x] || (/^\d+$/.test(String(x)) ? `${x} channels` : x));

export default function playback(ctx) {
  ctx.title('Playback');
  let days = readDays(ctx.query);
  let userId = isAdmin() ? ctx.query.get('user_id') || '' : '';
  let metric = store.get('finstats.methodMetric', 'plays') === 'watch_s' ? 'watch_s' : 'plays';
  const view = h('div', { class: 'stack' });

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.cardBlock(80), h('div', { class: 'grid-3' }, sk.cardRows(5), sk.cardRows(5), sk.cardRows(5))],
    fetch: () => api.get('/stats/playback', { days, user_id: userId }, { signal: ctx.signal }),
    render: (d) => {
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

function methodsTable(methods) {
  const rows = methods || [];
  const total = rows.reduce((a, m) => a + (m.plays || 0), 0);
  return h('div', { class: 'table-scroll chart-table' }, h('table', { class: 'table' },
    h('thead', null, h('tr', null, h('th', null, 'Method'), h('th', { class: 'r' }, 'Plays'), h('th', { class: 'r' }, 'Share'), h('th', { class: 'r' }, 'Watch time'))),
    h('tbody', null, rows.map((m) => h('tr', null, h('td', null, humanize(m.name)), h('td', { class: 'mono r' }, num(m.plays)),
      h('td', { class: 'mono r' }, total ? pct(m.plays / total, 1) : '–'), h('td', { class: 'mono r' }, duration(m.watch_s)))))));
}
