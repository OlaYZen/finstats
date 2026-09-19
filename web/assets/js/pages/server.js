// /server — what the Jellyfin server itself looks like (GET /api/server, admins only).

import { h, icon, num, bytes, duration, relEl, dateTime, debounce, mount } from '../dom.js';
import { api, isAbort } from '../api.js';
import { state } from '../state.js';
import { pageHeader, card, dataView, sk, emptyState, facts, setBusy, inlineError } from '../components.js';

const RESULT = {
  Completed: ['sev-good', 'check', 'Completed'],
  Failed: ['sev-critical', 'alert', 'Failed'],
  Aborted: ['sev-warning', 'alert', 'Aborted'],
  Cancelled: ['sev-warning', 'alert', 'Cancelled'],
};

export default function serverPage(ctx) {
  ctx.title('Server');
  const headerSlot = h('div', null, pageHeader('Server', 'Your Jellyfin server, as finstats last saw it'));
  const view = h('div', { class: 'stack' });
  let deviceFilter = '';

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.cardBlock(120), h('div', { class: 'grid-2' }, sk.cardRows(4), sk.cardRows(4)), sk.cardBlock(200)],
    fetch: () => api.get('/server', null, { signal: ctx.signal }),
    render,
  });

  function render(d) {
    d = d || {};
    const info = d.info || null;
    const name = (info && info.server_name) || (state.status && state.status.server_name) || 'Jellyfin';
    const chips = [
      info && info.has_update_available ? h('span', { class: 'sev sev-info chip-like' }, icon('arrowUp', 13), 'Update available') : null,
      info && info.has_pending_restart ? h('span', { class: 'sev sev-warning chip-like' }, icon('refresh', 13), 'Restart pending') : null,
    ].filter(Boolean);
    headerSlot.replaceChildren(pageHeader(name,
      [info && info.version ? `Jellyfin ${info.version}` : 'Your Jellyfin server', d.fetched_at ? ['· details from ', relEl(d.fetched_at, '')] : null],
      chips.length ? h('div', { class: 'chips' }, chips) : null));

    if (!d.fetched_at) return fetchPrompt();

    return [
      info ? card({ title: 'System', body: facts([
        ['Version', info.version, { mono: true }],
        ['Operating system', [info.operating_system, info.architecture].filter(Boolean).join(' · ') || null],
        ['Transcoder', info.encoder_location],
        info.transcoding_temp_path ? ['Transcode folder', h('span', { class: 'mono path' }, info.transcoding_temp_path)] : null,
        info.cache_path ? ['Cache folder', h('span', { class: 'mono path' }, info.cache_path)] : null,
        info.program_data_path ? ['Data folder', h('span', { class: 'mono path' }, info.program_data_path)] : null,
        info.log_path ? ['Log folder', h('span', { class: 'mono path' }, info.log_path)] : null,
      ]) }) : null,
      storageCard(d.storage),
      devicesCard(d.devices),
      h('div', { class: 'grid-2' }, pluginsCard(d.plugins), tasksCard(d.scheduled_tasks)),
    ];
  }

  // ---- nothing fetched yet
  function fetchPrompt() {
    const err = h('div');
    const btn = h('button', { type: 'button', class: 'btn btn-primary' }, icon('refresh', 14), 'Fetch server details');
    btn.addEventListener('click', async () => {
      mount(err);
      setBusy(btn, true, 'Fetching…');
      try {
        await api.post('/tasks/sync_server/run', {}, { signal: ctx.signal });
      } catch (e) {
        if (isAbort(e) || e.status === 401) return;
        if (e.status !== 409) { setBusy(btn, false); mount(err, inlineError('server-fetch-err', e.message)); return; } // 409 = already running: just wait for it
      }
      const deadline = Date.now() + 30000;
      const poll = async () => {
        if (ctx.signal.aborted) return;
        try {
          const d = await api.get('/server', null, { signal: ctx.signal });
          if (d && d.fetched_at) { dv.load(); return; }
        } catch (e) { if (isAbort(e) || e.status === 401) return; }
        if (Date.now() > deadline) {
          setBusy(btn, false);
          mount(err, inlineError('server-fetch-err', 'Jellyfin hasn’t answered yet. Check Settings → Tasks for the reason, then try again.'));
          return;
        }
        timer = setTimeout(poll, 2000);
      };
      timer = setTimeout(poll, 2000);
    });
    return emptyState('No server details yet', 'finstats fetches version, storage, plugins, scheduled tasks and devices from Jellyfin on its regular sync. You can also fetch them now.',
      h('div', { class: 'empty-action' }, btn, err));
  }
  let timer = null;
  ctx.onCleanup(() => clearTimeout(timer));

  // ---- storage
  function storageCard(storage) {
    const rows = (Array.isArray(storage) ? storage : []).filter((x) => x && x.used_bytes >= 0 && x.free_bytes >= 0 && x.used_bytes + x.free_bytes > 0);
    if (!rows.length) {
      return card({ title: 'Storage', body: h('p', { class: 'help' }, 'This Jellyfin version doesn’t report disk usage (it arrived in 10.11).') });
    }
    const order = { library: 0, system: 1 };
    rows.sort((a, b) => (order[a.kind] ?? 2) - (order[b.kind] ?? 2));
    return card({ title: 'Storage', sub: 'Disk usage of the volumes Jellyfin lives on. Entries with identical numbers share a disk.',
      body: h('ul', { class: 'storage' }, rows.map((x) => {
        const total = x.used_bytes + x.free_bytes, share = x.used_bytes / total;
        // Severity lives in the fill; the track is a lighter step of the same hue.
        const level = share >= 0.95 ? 'is-critical' : share >= 0.85 ? 'is-warning' : '';
        return h('li', { class: 'storage-row' },
          h('div', { class: 'storage-head' },
            h('span', { class: 'storage-label' }, x.label || 'Volume', x.kind === 'library' ? h('span', { class: 'chip' }, 'Library') : null),
            h('span', { class: 'mono storage-nums' }, `${bytes(x.free_bytes)} free of ${bytes(total)}`)),
          h('div', { class: ['meter meter-block storage-meter', level], role: 'progressbar', 'aria-label': `${x.label || 'Volume'} disk usage`, 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': Math.round(share * 100) },
            h('span', { class: 'meter-fill', style: { width: Math.max(1, share * 100) + '%' } })),
          h('div', { class: 'storage-foot' }, x.path ? h('span', { class: 'mono path' }, x.path) : h('span'),
            level ? h('span', { class: ['sev', level === 'is-critical' ? 'sev-critical' : 'sev-warning'] }, icon('alert', 13), level === 'is-critical' ? 'Almost full' : 'Getting full')
                  : h('span', { class: 'mono muted' }, Math.round(share * 100) + '% used')));
      })) });
  }

  // ---- devices (client-side filter; the list is small)
  function devicesCard(devices) {
    const all = Array.isArray(devices) ? devices : [];
    const body = h('div');
    const count = h('span', { class: 'mono muted' });
    const paint = () => {
      const q = deviceFilter.trim().toLowerCase();
      const rows = q ? all.filter((d) => [d.name, d.app, d.app_version, d.last_user_name].some((v) => v && String(v).toLowerCase().includes(q))) : all;
      count.textContent = q ? `${num(rows.length)} of ${num(all.length)}` : num(all.length);
      if (!all.length) return mount(body, emptyState('No devices known yet.'));
      if (!rows.length) return mount(body, emptyState('No devices match', 'Try a device, app or user name.'));
      mount(body, h('div', { class: 'table-scroll' }, h('table', { class: 'table' },
        h('thead', null, h('tr', null, h('th', null, 'Device'), h('th', null, 'App'), h('th', null, 'Last user'), h('th', null, 'Last seen'))),
        h('tbody', null, rows.map((d) => h('tr', null,
          h('td', { class: 'wrap-cell' }, d.name || 'Unknown device'),
          h('td', null, h('div', { class: 'client-cell' }, h('span', null, d.app || '–'), h('span', { class: 'muted mono' }, d.app_version || ''))),
          h('td', null, d.last_user_id ? h('a', { href: `/users/${d.last_user_id}` }, d.last_user_name || 'Unknown user') : d.last_user_name || h('span', { class: 'muted' }, '–')),
          h('td', null, d.last_seen ? relEl(d.last_seen) : h('span', { class: 'muted' }, '–'))))))));
    };
    const onInput = debounce((v) => { deviceFilter = v; paint(); }, 150);
    ctx.onCleanup(() => onInput.cancel());
    const input = h('input', { class: 'input input-search', type: 'search', placeholder: 'Filter devices…', 'aria-label': 'Filter devices', value: deviceFilter, spellcheck: false,
      onInput: (e) => onInput(e.target.value) });
    paint();
    return card({ title: ['Devices ', count], sub: 'Every device that has signed in to Jellyfin',
      actions: all.length > 5 ? h('div', { class: 'search-field search-field-sm' }, icon('search', 14), input) : null, cls: 'card-flush', body });
  }

  function pluginsCard(plugins) {
    const rows = Array.isArray(plugins) ? plugins : [];
    return card({ title: 'Plugins', cls: 'card-flush', body: !rows.length ? emptyState('No plugins reported.') :
      h('div', { class: 'table-scroll' }, h('table', { class: 'table' },
        h('thead', null, h('tr', null, h('th', null, 'Plugin'), h('th', null, 'Version'), h('th', null, 'Status'))),
        h('tbody', null, rows.map((p) => {
          const ok = !p.status || p.status === 'Active';
          return h('tr', null, h('td', { title: p.description || null }, p.name || '–'), h('td', { class: 'mono' }, p.version || '–'),
            h('td', null, h('span', { class: ['sev', ok ? 'sev-good' : 'sev-warning'] }, icon(ok ? 'check' : 'alert', 13), p.status || 'Active')));
        })))) });
  }

  function tasksCard(tasks) {
    const rows = Array.isArray(tasks) ? tasks : [];
    return card({ title: 'Scheduled tasks', sub: 'Jellyfin’s own background jobs', cls: 'card-flush', body: !rows.length ? emptyState('No scheduled tasks reported.') :
      h('div', { class: 'table-scroll' }, h('table', { class: 'table' },
        h('thead', null, h('tr', null, h('th', null, 'Task'), h('th', null, 'State'), h('th', null, 'Last result'), h('th', null, 'Last run'), h('th', { class: 'r' }, 'Took'))),
        h('tbody', null, rows.map((t) => {
          const [cls, ic, label] = RESULT[t.last_result] || ['sev-info', 'info', t.last_result || null];
          return h('tr', null,
            h('td', null, h('div', { class: 'client-cell' }, h('span', null, t.name || '–'), h('span', { class: 'muted' }, t.category || ''))),
            h('td', null, t.state && t.state !== 'Idle' ? h('span', { class: 'badge live' }, h('span', { class: 'badge-dot' }), t.state) : h('span', { class: 'muted' }, t.state || '–')),
            h('td', null, label ? h('span', { class: 'sev ' + cls }, icon(ic, 13), label) : h('span', { class: 'muted' }, 'Never ran')),
            h('td', null, t.last_run_at ? relEl(t.last_run_at) : h('span', { class: 'muted' }, '–')),
            h('td', { class: 'mono r' }, t.last_duration_s != null ? duration(t.last_duration_s) : '–'));
        })))) });
  }

  ctx.root.append(headerSlot, view);
  dv.load();
}
