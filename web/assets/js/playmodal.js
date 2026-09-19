// Detail modal for one play (GET /api/activity/{id}).

import { h, icon, duration, durationExact, dateTime, clock, bitrate, pct, humanize, episodeCode, mount } from './dom.js';
import { api, isAbort } from './api.js';
import { isAdmin } from './state.js';
import { openModal, copyButton, methodBadge, facts, errorState, sk, poster, setBusy, inlineError } from './components.js';

const res = (w, hgt) => (w && hgt ? `${w}×${hgt}` : null);
const channels = (n) => (n == null ? null : { 1: 'Mono', 2: 'Stereo', 6: '5.1', 8: '7.1' }[n] || `${n} ch`);

export function openPlayModal(play, { onDeleted } = {}) {
  const abort = new AbortController();
  const body = h('div', { class: 'play-modal' }, sk.rows(3));
  const modal = openModal({ title: 'Play details', body, wide: true, onClose: () => abort.abort() });

  async function load() {
    try {
      const p = await api.get(`/activity/${play.id}`, null, { signal: abort.signal });
      mount(body, render(p));
    } catch (e) {
      if (isAbort(e) || e.status === 401) return;
      mount(body, errorState(e, () => { mount(body, sk.rows(3)); load(); }));
    }
  }

  function render(p) {
    const code = episodeCode(p.season_number, p.episode_number);
    const canLink = p.item_exists !== false;
    const close = () => modal.close();
    const titleMain = p.series_name || p.item_name || 'Unknown item';
    const head = h('div', { class: 'play-head' },
      poster(p.image_item_id, titleMain, { w: 300, cls: 'poster-md' }),
      h('div', { class: 'play-head-text' },
        h('div', { class: 'play-head-title' }, canLink && (p.series_id || p.item_id) ? h('a', { href: `/items/${p.series_id || p.item_id}`, onClick: close }, titleMain) : titleMain),
        p.series_name ? h('div', { class: 'play-head-sub' }, code ? h('span', { class: 'mono' }, code) : null, code ? ' · ' : null,
          canLink ? h('a', { href: `/items/${p.item_id}`, onClick: close }, p.item_name) : p.item_name) : null,
        h('div', { class: 'play-head-meta' }, methodBadge(p.play_method),
          p.active ? h('span', { class: 'badge live' }, h('span', { class: 'badge-dot' }), 'Playing now') : null,
          h('span', { class: 'chip' }, p.source === 'jellystat' ? 'Imported from Jellystat' : 'Recorded by finstats'),
          canLink ? null : h('span', { class: 'chip' }, 'No longer in library'))));

    const session = facts([
      ['User', h('a', { href: `/users/${p.user_id}`, onClick: close }, p.user_name)],
      ['Started', dateTime(p.started_at), { mono: true }],
      ['Ended', p.active ? 'Still playing' : dateTime(p.ended_at), { mono: true }],
      ['Watched', h('span', { title: durationExact(p.duration_s) }, duration(p.duration_s)), { mono: true }],
      ['Paused for', p.paused_s ? duration(p.paused_s) : '–', { mono: true }],
      ['Stopped at', p.position_s != null ? `${clock(p.position_s)}${p.runtime_s ? ' / ' + clock(p.runtime_s) : ''}${p.completion != null ? ` (${pct(Math.min(1, p.completion))})` : ''}` : '–', { mono: true }],
    ]);

    const device = facts([
      ['Client', [p.client, p.app_version].filter(Boolean).join(' ') || '–'],
      ['Device', p.device_name],
      p.device_id ? ['Device ID', h('span', { class: 'copy-row' }, h('span', { class: 'mono trunc' }, p.device_id), copyButton(p.device_id, 'Copy device ID'))] : null,
      isAdmin() ? ['IP address', p.remote_ip ? h('span', { class: 'copy-row' }, h('span', { class: 'mono' }, p.remote_ip), copyButton(p.remote_ip, 'Copy IP address')) : '–'] : null,
    ]);

    const media = facts([
      ['Container', p.container, { mono: true }],
      ['Bitrate', bitrate(p.bitrate), { mono: true }],
      ['Video', [p.video_codec && p.video_codec.toUpperCase(), res(p.width, p.height), p.video_range, p.bit_depth ? p.bit_depth + '-bit' : null].filter(Boolean).join(' · ') || p.video, { mono: true }],
      ['Audio', [p.audio_codec && p.audio_codec.toUpperCase(), channels(p.audio_channels), p.audio_language].filter(Boolean).join(' · ') || p.audio, { mono: true }],
      ['Subtitles', [p.subtitle_language, p.subtitle_codec].filter(Boolean).join(' · ') || p.subtitle || 'Off', { mono: true }],
    ]);

    const t = p.transcode;
    const transcode = t ? [h('h3', { class: 'section-label' }, 'Transcoding'), facts([
      ['Video', t.is_video_direct ? 'Copied (direct)' : [t.video_codec && t.video_codec.toUpperCase(), res(t.width, t.height)].filter(Boolean).join(' · '), { mono: true }],
      ['Audio', t.is_audio_direct ? 'Copied (direct)' : [t.audio_codec && t.audio_codec.toUpperCase(), channels(t.audio_channels)].filter(Boolean).join(' · '), { mono: true }],
      ['Container', t.container, { mono: true }],
      ['Bitrate', bitrate(t.bitrate), { mono: true }],
      ['Hardware', t.hw_accel ? t.hw_accel.toUpperCase() : 'Software', { mono: true }],
      ['Reasons', t.reasons && t.reasons.length ? h('span', { class: 'chips' }, t.reasons.map((r) => h('span', { class: 'chip' }, humanize(r)))) : '–'],
    ])] : null;

    return [head,
      h('h3', { class: 'section-label' }, 'Session'), session,
      h('h3', { class: 'section-label' }, 'Device'), device,
      h('h3', { class: 'section-label' }, 'Media'), media,
      transcode,
      isAdmin() && !p.active ? deleteRow(p) : null];
  }

  // Destructive action: inline two-step inside the modal, explicit Cancel/Delete.
  function deleteRow(p) {
    const row = h('div', { class: 'danger-row' });
    const idle = () => mount(row, h('button', { type: 'button', class: 'btn btn-ghost btn-sm btn-danger-text', onClick: confirm }, icon('trash', 14), 'Delete this play'));
    function confirm() {
      const err = h('div');
      const del = h('button', { type: 'button', class: 'btn btn-sm btn-danger' }, 'Delete play');
      const cancel = h('button', { type: 'button', class: 'btn btn-sm', onClick: () => { idle(); row.querySelector('button').focus(); } }, 'Cancel');
      del.addEventListener('click', async () => {
        setBusy(del, true, 'Deleting…'); cancel.disabled = true;
        try {
          await api.del(`/activity/${p.id}`);
          modal.close();
          if (onDeleted) onDeleted(p);
        } catch (e) {
          setBusy(del, false); cancel.disabled = false;
          mount(err, inlineError('del-err', e.message));
        }
      });
      mount(row, h('div', { class: 'confirm', role: 'group', 'aria-label': 'Confirm delete' },
        h('p', null, 'Delete this play from your stats? This can’t be undone.'), h('div', { class: 'confirm-btns' }, cancel, del)), err);
      del.focus();
    }
    idle();
    return row;
  }

  load();
  return modal;
}
