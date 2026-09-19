// Larger blocks shared by several pages.

import { h, icon, num, compact, duration, durationExact, clock, bitrate, humanize, episodeCode, store, mount } from './dom.js';
import { columnsChart, columnsTable, heatmap, heatmapTable, sparkline } from './charts.js';
import { chartCard, segmented, statTile, poster, avatar, methodBadge, facts } from './components.js';
import { isAdmin, rangeLong } from './state.js';

const METRICS = [{ value: 'watch_s', label: 'Watch time' }, { value: 'plays', label: 'Plays' }];

function metricPref() { const m = store.get('finstats.metric', 'watch_s'); return m === 'plays' ? 'plays' : 'watch_s'; }

/** Stacked columns by media type, with a Watch time | Plays view switch. */
export function activityCard({ daily, bucket, title = 'Activity' }) {
  let metric = metricPref();
  const el = chartCard({
    title,
    sub: bucket === 'week' ? 'Per week, by media type' : 'Per day, by media type',
    controls: segmented({ label: 'Measure', size: 'seg-sm', value: metric, options: METRICS,
      onChange: (v) => { metric = v; store.set('finstats.metric', v); el.rerender(); } }),
    chart: () => columnsChart({ daily, bucket, metric }),
    table: () => columnsTable({ daily, bucket }),
  });
  return el;
}

export function heatmapCard({ data, title = 'When people watch' }) {
  return chartCard({
    title, sub: 'Plays by weekday and hour, server time',
    chart: () => heatmap({ data, metric: 'plays' }),
    table: () => heatmapTable({ data }),
  });
}

export function overviewTiles({ totals, previous, daily, days, scoped }) {
  const vs = previous ? `vs previous ${days === 365 ? 'year' : days + ' days'}` : null;
  const t = totals || {}, p = previous || null;
  return h('div', { class: 'tiles' },
    statTile({ label: 'Watch time', value: duration(t.watch_s), title: durationExact(t.watch_s), current: t.watch_s, previous: p && p.watch_s, vsLabel: vs,
      hint: rangeLong(days), spark: sparkline((daily || []).map((d) => d.watch_s)) }),
    statTile({ label: 'Plays', value: compact(t.plays), title: num(t.plays), current: t.plays, previous: p && p.plays, vsLabel: vs,
      hint: rangeLong(days), spark: sparkline((daily || []).map((d) => d.plays)) }),
    scoped ? null : statTile({ label: 'Active users', value: compact(t.active_users), title: num(t.active_users), current: t.active_users, previous: p && p.active_users, vsLabel: vs, hint: rangeLong(days) }),
    statTile({ label: 'Different titles played', value: compact(t.distinct_items), title: num(t.distinct_items), current: t.distinct_items, previous: p && p.distinct_items, vsLabel: vs, hint: rangeLong(days) }));
}

// ---------------------------------------------------------------- now playing
const openTranscode = new Set(); // keeps <details> open across polls

export function nowPlayingCard(sn) {
  const code = episodeCode(sn.season_number, sn.episode_number);
  const prog = sn.runtime_s ? Math.max(0, Math.min(1, (sn.position_s || 0) / sn.runtime_s)) : null;
  const title = sn.series_name || sn.item_name;
  const t = sn.transcode;

  let details = null;
  if (t) {
    details = h('details', { class: 'np-details' },
      h('summary', null, icon('chevronRight', 13), 'Transcode details'),
      facts([
        ['Video', t.is_video_direct ? 'Copied (direct)' : (t.video_codec || '–').toUpperCase(), { mono: true }],
        ['Audio', t.is_audio_direct ? 'Copied (direct)' : (t.audio_codec || '–').toUpperCase(), { mono: true }],
        ['Container', t.container, { mono: true }],
        ['Hardware', t.hw_accel ? t.hw_accel.toUpperCase() : 'Software', { mono: true }],
        t.progress != null ? ['Transcoded', Math.round(Math.min(1, t.progress) * 100) + '%', { mono: true }] : null,
        ['Reasons', t.reasons && t.reasons.length ? t.reasons.map(humanize).join(', ') : '–'],
      ]));
    details.open = openTranscode.has(sn.key);
    details.addEventListener('toggle', () => { if (details.open) openTranscode.add(sn.key); else openTranscode.delete(sn.key); });
  }

  return h('article', { class: ['np', sn.is_paused && 'is-paused'] },
    poster(sn.image_item_id, title, { w: 300, cls: 'poster-np' }),
    h('div', { class: 'np-main' },
      h('div', { class: 'np-title' }, h('a', { href: `/items/${sn.series_id || sn.item_id}` }, title)),
      sn.series_name ? h('div', { class: 'np-sub' }, code ? h('span', { class: 'mono' }, code) : null, code ? ' · ' : null, sn.item_name) : null,
      h('div', { class: 'np-user' }, avatar(sn.user_id, sn.user_name, { size: 20 }), h('a', { href: `/users/${sn.user_id}` }, sn.user_name),
        h('span', { class: 'muted' }, ' · ', [sn.client, sn.device_name].filter(Boolean).join(' on '))),
      h('div', { class: 'np-badges' },
        sn.is_paused ? h('span', { class: 'badge paused' }, icon('pause', 11), 'Paused') : h('span', { class: 'badge live' }, h('span', { class: 'badge-dot' }), 'Playing'),
        methodBadge(sn.play_method),
        [sn.video, sn.audio].filter(Boolean).map((x) => h('span', { class: 'chip mono' }, x)),
        sn.bitrate ? h('span', { class: 'chip mono' }, bitrate(sn.bitrate)) : null,
        isAdmin() && sn.remote_ip ? h('span', { class: 'chip mono', title: 'IP address' }, sn.remote_ip) : null),
      h('div', { class: 'np-progress' },
        h('div', { class: 'meter meter-wide', role: 'progressbar', 'aria-label': 'Playback position', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': prog == null ? null : Math.round(prog * 100) },
          h('span', { class: 'meter-fill', style: { width: (prog || 0) * 100 + '%' } })),
        h('span', { class: 'mono np-time' }, sn.runtime_s ? `${clock(sn.position_s)} / ${clock(sn.runtime_s)}` : clock(sn.position_s))),
      details));
}

export function nowPlayingList(sessions) {
  if (!sessions.length) return h('p', { class: 'np-empty' }, 'Nothing is playing right now.');
  // forget <details> state for sessions that ended
  const keys = new Set(sessions.map((x) => x.key));
  for (const k of openTranscode) if (!keys.has(k)) openTranscode.delete(k);
  return h('div', { class: 'np-grid' }, sessions.map(nowPlayingCard));
}
