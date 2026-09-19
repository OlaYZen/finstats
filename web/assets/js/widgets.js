// Larger blocks shared by several pages.

import { h, icon, num, compact, bytes, duration, durationExact, clock, bitrate, humanize, episodeCode, store, mount, relTime, dateTime, dayLabelLong } from './dom.js';
import { columnsChart, columnsTable, heatmap, heatmapTable, sparkline, bucketList, libBucketList, simpleColumns, simpleColumnsTable } from './charts.js';
import { card, chartCard, segmented, statTile, poster, avatar, methodBadge, facts } from './components.js';
import { rangeLong, can } from './state.js';

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
        can('see_network') && sn.remote_ip ? h('span', { class: 'chip mono', title: 'IP address' }, sn.remote_ip) : null),
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

// ---------------------------------------------------------------- insights (GET /api/stats/insights)
const upper = (x) => (x && String(x).length <= 6 ? String(x).toUpperCase() : x);

/** The quieter second row of dashboard tiles. Returns null when there is nothing to say. */
export function insightTiles(ins) {
  if (!ins) return null;
  const c = ins.concurrency || {};
  const net = Array.isArray(ins.network) ? ins.network : [];
  const netTotal = net.reduce((a, b) => a + (b.plays || 0), 0);
  const remote = (net.find((b) => b.name === 'Remote') || {}).plays || 0;
  const tiles = [
    c.peak != null ? h('div', { class: 'tile tile-quiet' },
      h('div', { class: 'tile-label' }, 'Peak concurrent streams'),
      h('div', { class: 'tile-value' }, num(c.peak)),
      h('div', { class: 'tile-foot' }, h('span', { class: 'tile-vs' },
        c.peak_transcodes > 0 ? `${num(c.peak_transcodes)} transcoding at once` : c.peak_at ? h('span', { title: dateTime(c.peak_at) }, relTime(c.peak_at)) : ' '))) : null,
    ins.data_bytes != null ? h('div', { class: 'tile tile-quiet' },
      h('div', { class: 'tile-label' }, 'Data streamed'),
      h('div', { class: 'tile-value', title: num(ins.data_bytes) + ' bytes' }, bytes(ins.data_bytes)),
      h('div', { class: 'tile-foot' }, h('span', { class: 'tile-vs' }, 'estimated from stream bitrates'))) : null,
    can('see_network') && netTotal > 0 ? h('div', { class: 'tile tile-quiet' },
      h('div', { class: 'tile-label' }, 'Remote plays'),
      h('div', { class: 'tile-value' }, Math.round((remote / netTotal) * 100) + '%'),
      h('div', { class: 'tile-foot' }, h('span', { class: 'tile-vs' }, `${num(remote)} of ${num(netTotal)} plays`))) : null,
  ].filter(Boolean);
  return tiles.length ? h('div', { class: 'tiles tiles-quiet' }, tiles) : null;
}

export function genresCard(genres, { sub = 'By watch time' } = {}) {
  return card({ title: 'Genres', sub, body: bucketList(genres, { empty: 'No genre information for these plays yet.' }) });
}

/** Admin-only; hidden entirely when there is nothing to show. */
export function failedLoginsCard(rows) {
  if (!can('see_server') || !Array.isArray(rows) || !rows.length) return null;
  return card({ title: 'Failed sign-ins', sub: 'Most recent attempts on your Jellyfin server',
    actions: h('a', { class: 'btn btn-ghost btn-sm', href: '/events' }, 'Server log', icon('chevronRight', 14)),
    body: h('ul', { class: 'mini-list' }, rows.map((r) => h('li', { class: 'mini-row' },
      h('span', { class: 'sev sev-warning' }, icon('alert', 13)),
      h('span', { class: 'mini-main' }, r.overview || 'Failed sign-in', r.user_name ? h('span', { class: 'muted' }, ' · ' + r.user_name) : null),
      h('time', { class: 'mono muted mini-when', title: dateTime(r.date) }, relTime(r.date))))) });
}

// ---------------------------------------------------------------- library make-up (GET /api/library/insights)
const monthf = new Intl.DateTimeFormat(undefined, { month: 'short', year: '2-digit' });
const monthLong = new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' });
function monthDate(str) { const [y, m] = String(str).split('-').map(Number); return new Date(y || 1970, (m || 1) - 1, 1); }

function libItemRows(items, { empty, showAdded = false }) {
  if (!items || !items.length) return h('div', { class: 'chart-empty chart-empty-sm' }, empty);
  return h('ol', { class: 'toplist' }, items.map((it, i) => h('li', { class: 'toplist-row' },
    h('span', { class: 'toplist-rank mono' }, String(i + 1)),
    poster(it.image_item_id || it.id, it.name, { w: 120, cls: 'poster-sm' }),
    h('div', { class: 'toplist-main' }, it.id ? h('a', { href: `/items/${it.id}`, class: 'toplist-name' }, it.name) : h('span', { class: 'toplist-name' }, it.name),
      h('div', { class: 'toplist-sub' }, [it.year, it.type === 'Series' ? 'Series' : null, showAdded && it.date_created ? 'added ' + relTime(it.date_created) : null].filter(Boolean).join(' · ') || ' ')),
    h('div', { class: 'toplist-nums' }, h('span', { class: 'mono toplist-watch' }, it.size_bytes ? bytes(it.size_bytes) : '–')))));
}

/**
 * "What your library is made of". Not scoped by the time range — it describes the files,
 * so it renders under its own heading, away from the range-filtered cards.
 */
export function libraryInsights(d, { scoped = false } = {}) {
  if (!d) return null;
  const t = d.totals || {};
  if (!(t.files > 0) && !(d.resolutions || []).length && !(d.largest || []).length) {
    return h('section', { class: 'subsection' }, h('h2', { class: 'subsection-title' }, scoped ? 'What this library is made of' : 'What your library is made of'),
      h('p', { class: 'np-empty' }, 'File details appear after the next library sync.'));
  }
  const b = (title, sub, rows, labelFn, unit) => card({ title, sub, body: libBucketList(rows, { labelFn, unit }) });
  const decades = (d.decades || []).map((x) => ({ label: x.name, title: x.name, value: x.count }));
  const added = (d.added || []).map((x) => ({ label: monthf.format(monthDate(x.month)), title: monthLong.format(monthDate(x.month)), value: x.count }));
  const un = d.unwatched || null;
  const counts = [['Movies', t.movies], ['Series', t.series], ['Episodes', t.episodes], ['Tracks', t.tracks]].filter(([, v]) => v > 0);
  return h('section', { class: 'subsection stack' },
    h('div', null, h('h2', { class: 'subsection-title' }, scoped ? 'What this library is made of' : 'What your library is made of'),
      h('p', { class: 'subsection-sub' }, 'About the files themselves — the time range above doesn’t apply here.')),
    h('div', { class: 'tiles tiles-3' },
      h('div', { class: 'tile' }, h('div', { class: 'tile-label' }, 'Files'), h('div', { class: 'tile-value' }, compact(t.files)),
        h('div', { class: 'tile-foot' }, h('span', { class: 'tile-vs' }, counts.map(([k, v]) => `${num(v)} ${k.toLowerCase()}`).join(' · ') || ' '))),
      h('div', { class: 'tile' }, h('div', { class: 'tile-label' }, 'Total size'), h('div', { class: 'tile-value' }, bytes(t.size_bytes)),
        h('div', { class: 'tile-foot' }, h('span', { class: 'tile-vs' }, t.files > 0 && t.size_bytes > 0 ? `${bytes(t.size_bytes / t.files)} per file on average` : ' '))),
      h('div', { class: 'tile' }, h('div', { class: 'tile-label' }, 'Total runtime'), h('div', { class: 'tile-value', title: durationExact(t.runtime_s) }, duration(t.runtime_s)),
        h('div', { class: 'tile-foot' }, h('span', { class: 'tile-vs' }, 'to play everything once')))),
    h('div', { class: 'grid-3' },
      b('Resolutions', 'Video files', d.resolutions),
      b('Video codecs', 'Video files', d.video_codecs, upper),
      b('Dynamic range', 'Video files', d.video_ranges)),
    h('div', { class: 'grid-3' },
      b('Containers', 'All files', d.containers, upper),
      b('Audio codecs', 'First audio track', d.audio_codecs, upper),
      b('Genres', 'Movies and series', d.genres, undefined, 'Titles')),
    h('div', { class: 'grid-2' },
      chartCard({ title: 'By decade', sub: 'Titles by release year',
        chart: () => simpleColumns({ rows: decades, unit: ['title', 'titles'], ariaLabel: 'Titles per decade' }),
        table: () => simpleColumnsTable({ rows: decades, head: ['Decade', 'Titles'] }) }),
      chartCard({ title: 'Added per month', sub: 'Last 24 months',
        chart: () => simpleColumns({ rows: added, unit: ['title', 'titles'], ariaLabel: 'Titles added per month' }),
        table: () => simpleColumnsTable({ rows: added, head: ['Month', 'Added'] }) })),
    h('div', { class: 'grid-2' },
      card({ title: 'Largest', sub: 'Series count all their episodes', body: libItemRows(d.largest, { empty: 'No file sizes known yet.' }) }),
      card({ title: 'Never watched',
        sub: un && un.count > 0 ? `${num(un.count)} ${un.count === 1 ? 'title' : 'titles'} · ${bytes(un.size_bytes)} nobody has played` : 'Everything has been played at least once',
        body: [libItemRows(un && un.items, { empty: 'Nothing unwatched — or no file sizes known yet.', showAdded: true }),
          h('p', { class: 'help card-note' }, 'Combines plays recorded by finstats with Jellyfin’s own played flags, so history from before finstats counts too.')] })));
}
