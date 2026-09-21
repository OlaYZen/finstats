import { h, icon, num, bytes, relTime, dateTime, mount, humanize } from '../dom.js';
import { api, isAbort, uploadRaw } from '../api.js';
import { isAdmin, state } from '../state.js';
import { pageHeader, card, sk, toggle, setBusy, inlineError, errorState, facts, spinner, avatar } from '../components.js';
import { connectionsPanel } from '../connections.js';
import { dataTable, plainTable } from '../tables.js';

const SERVICE_TASKS = { sync_upcoming: 'upcoming' };   // task → the feature it belongs to
const TASK_LABEL = {
  sync_users: ['Sync users', 'Names, roles and last-seen times from Jellyfin'],
  sync_libraries: ['Read libraries and items', 'Copies titles and file details from Jellyfin. Read-only: it never starts a scan on Jellyfin'],
  sync_events: ['Sync server log', 'Jellyfin’s activity log: sign-ins, failed logins, tasks'],
  sync_server: ['Server details', 'Version, storage, plugins, scheduled tasks and devices'],
  sync_userdata: ['Watched & favourites', 'Per-user played flags and favourites from Jellyfin'],
  import: ['Jellystat import', 'Runs when you upload a backup below'],
  backup: ['Backup', 'Writes a finstats backup. Runs by itself on the schedule under Backups'],
  restore: ['Restore', 'Runs when you restore a finstats backup under Backups'],
  sync_upcoming: ['Read Sonarr and Radarr calendars', 'What is about to air or be released. Read-only, every 15 minutes'],
  geoip: ['Geolocation database', 'Downloads the city database the Security page places addresses with. Started under Security below'],
};
/** "in 6 days" — relTime only looks backwards. */
function untilText(ts) {
  const s = ts - Date.now() / 1000;
  if (s <= 90) return 'within a minute or two';
  if (s < 3600) return `in ${Math.round(s / 60)} minutes`;
  if (s < 86400 * 1.5) return `in ${Math.round(s / 3600)} hours`;
  return `in ${Math.round(s / 86400)} days`;
}

const OWN_CARD = new Set(['import', 'backup', 'restore']); // started from their own section, not with “Run now”

// The upload lives outside the page so it keeps going if you navigate away.
const upload = { active: false, progress: 0, loaded: 0, total: 0, fileName: '', error: null, doneAt: 0, handle: null };
const uploadSubs = new Set();
const notifyUpload = () => uploadSubs.forEach((fn) => fn());

function startUpload(file) {
  Object.assign(upload, { active: true, progress: 0, loaded: 0, total: file.size, fileName: file.name, error: null, doneAt: 0 });
  notifyUpload();
  const handle = uploadRaw('/import/jellystat', file, (p, loaded, total) => { Object.assign(upload, { progress: p, loaded, total }); notifyUpload(); });
  upload.handle = handle;
  handle.promise.then(() => { Object.assign(upload, { active: false, doneAt: Date.now(), handle: null }); notifyUpload(); })
    .catch((e) => {
      const msg = e.status === -1 ? null
        : e.status === 409 ? 'An import is already running. Wait for it to finish, then try again.'
        : e.status === 413 ? 'The server rejected the file as too large. If finstats sits behind a reverse proxy, raise its upload limit (for nginx: client_max_body_size) and try again.'
        : e.message;
      Object.assign(upload, { active: false, error: msg, handle: null }); notifyUpload();
    });
}

export default function settings(ctx) {
  ctx.title('Settings');
  const connSlot = h('div', null, sk.rows(2));
  const accessSlot = h('div', null, sk.rows(1));
  const collectSlot = h('div', null, sk.rows(3));
  const networkSlot = h('div', { class: 'net-stack' }, sk.rows(2));
  const securitySlot = h('div', { class: 'net-stack' }, sk.rows(2));
  const tasksSlot = h('div', null, sk.rows(3));
  const importSlot = h('div');
  const dbSlot = h('div', null, sk.rows(1));
  const backupsSlot = h('div', { class: 'net-stack' }, sk.rows(2));
  const outboundSlot = h('div', { class: 'net-stack' }, sk.rows(3));

  ctx.root.append(pageHeader('Settings', 'Connection, access, collection and data'),
    h('div', { class: 'stack settings' },
      card({ title: 'Jellyfin connection', body: connSlot }),
      isAdmin() ? card({ title: 'Connections', sub: 'Sonarr, Radarr, Seerr and torrent clients: what is requested, coming and downloading', body: connectionsPanel(ctx), id: 'connections' }) : null,
      card({ title: 'Access', sub: 'Who can use finstats and what they can see', body: accessSlot }),
      card({ title: 'Collection', sub: 'How finstats gathers data from Jellyfin', body: collectSlot }),
      card({ title: 'Home network', sub: 'Which plays count as local and which as remote', body: networkSlot }),
      card({ title: 'Security', sub: 'Where addresses are, and what counts as impossible travel', body: securitySlot, id: 'security' }),
      isAdmin() ? card({ title: 'Outbound connections', sub: 'Everywhere finstats can reach, and whether it is switched on', body: outboundSlot, id: 'outbound' }) : null,
      card({ title: 'Tasks', body: tasksSlot }),
      isAdmin() ? card({ title: 'Backups', sub: 'Your history, settings and permissions in one file, to keep safe or to move to another finstats', body: backupsSlot, id: 'backups' }) : null,
      card({ title: 'Import from Jellystat', sub: 'Bring your playback history with you', body: importSlot, id: 'import' }),
      card({ title: 'Database', body: dbSlot })));

  // ------------------------------------------------------------ settings
  let settingsData = null;
  async function loadSettings() {
    try {
      settingsData = await api.get('/settings', null, { signal: ctx.signal });
      renderAccess(); renderCollect(); renderNetwork(); renderSecurity(); renderConn(); loadBackups(); loadOutbound();
    } catch (e) {
      if (isAbort(e) || e.status === 401) return;
      mount(accessSlot, errorState(e, loadSettings)); mount(collectSlot, ''); mount(networkSlot, ''); mount(connSlot, '');
    }
  }

  let tasksData = null;
  function renderConn() {
    if (!settingsData) return;
    const c = tasksData && tasksData.collector;
    const status = !c ? h('span', { class: 'muted' }, 'Checking…')
      : c.connected ? h('span', { class: 'sev sev-good' }, icon('check', 13), `Connected · ${num(c.active_sessions)} active ${c.active_sessions === 1 ? 'session' : 'sessions'}`)
      : h('span', { class: 'sev sev-critical' }, icon('alert', 13), 'Not connected' + (c.error ? ` — ${c.error}` : ''));
    // How the collector is being told, and — when the socket is switched on but not carrying — why not.
    const live = c && c.transport === 'socket';
    const how = !c || !c.connected ? null
      : live ? h('span', { class: 'sev sev-good' }, icon('activity', 13), 'Pushed by Jellyfin, live')
      : [h('span', null, `Asked for every ${num(settingsData.active_interval_s)} s while watching, every ${num(settingsData.idle_interval_s)} s otherwise`),
         c.socket_enabled && c.socket_error ? h('p', { class: 'help' }, `The live connection is not carrying: ${c.socket_error}. finstats keeps trying; nothing is missed meanwhile.`) : null];
    mount(connSlot, facts([
      ['Server', settingsData.server_name],
      ['Address', settingsData.jellyfin_url, { mono: true }],
      ['Jellyfin version', settingsData.server_version, { mono: true }],
      ['Collector', status],
      how ? ['How', how] : null,
      c && c.last_poll_at ? ['Last checked', h('span', { title: dateTime(c.last_poll_at) }, relTime(c.last_poll_at)), { mono: true }] : null,
      live && c.last_reconcile_at ? ['Last checked the hard way', h('span', { title: dateTime(c.last_reconcile_at) }, relTime(c.last_reconcile_at)), { mono: true }] : null,
    ]), h('p', { class: 'help' }, 'finstats talks to Jellyfin with its own API key, created during setup. To point finstats at another server, start it with a fresh data directory.'));
  }

  /** An immediate-effect setting: a switch that saves on change and confirms next to itself. */
  function toggleRow({ key, label, help, onSaved }) {
    const note = h('span', { class: 'saved-note', 'aria-live': 'polite' });
    const err = h('div');
    let noteTimer;
    const sw = toggle({ checked: !!settingsData[key], labelledby: `${key}-label`, describedby: `${key}-help`,
      onChange: async (next, revert) => {
        mount(err, ''); note.replaceChildren(spinner(12));
        try {
          settingsData = await api.put('/settings', { [key]: next });
          note.replaceChildren(icon('check', 13), 'Saved');
          if (onSaved) onSaved();
          clearTimeout(noteTimer); noteTimer = setTimeout(() => note.replaceChildren(), 2000);
        } catch (e) {
          revert(!next); note.replaceChildren();
          mount(err, inlineError(`${key}-err`, `Couldn’t save: ${e.message}`));
        }
      } });
    return [h('div', { class: 'setting-row' },
      h('div', null, h('div', { class: 'setting-label', id: `${key}-label` }, label), h('p', { class: 'help', id: `${key}-help` }, help)),
      h('div', { class: 'setting-control' }, note, sw)), err];
  }

  // ------------------------------------------------------------ outbound connections
  // The privacy promise in the README, assembled from what the running program knows. Nothing is
  // recorded for this list: every line is read from something finstats already kept.
  const STATE_LABEL = { always: ['is-always', 'Always on'], on: ['is-on', 'On'], off: ['', 'Off'] };
  async function loadOutbound() {
    if (!isAdmin()) return;
    try {
      const data = await api.get('/outbound', null, { signal: ctx.signal });
      const rows = (data.destinations || []).map((d) => {
        const [cls, label] = STATE_LABEL[d.state] || STATE_LABEL.off;
        return h('div', { class: 'out-row' },
          h('div', { class: 'out-head' },
            h('span', { class: 'out-what' }, d.what),
            h('span', { class: 'out-state ' + cls }, label)),
          d.hosts && d.hosts.length ? h('div', { class: 'out-hosts mono' }, d.hosts.join(', ')) : null,
          h('p', { class: 'help' }, d.why),
          d.last_at ? h('p', { class: 'help' }, ['Last answered ', h('span', { title: dateTime(d.last_at) }, relTime(d.last_at))]) : null,
          d.error ? h('p', { class: 'help' }, `Last attempt failed: ${d.error}`) : null);
      });
      const off = (data.total || 0) - (data.reachable || 0);
      mount(outboundSlot, rows,
        h('p', { class: 'help' }, `${num(data.reachable || 0)} of ${num(data.total || 0)} switched on${off ? `, ${num(off)} off` : ''}. finstats never sends anything about you or your server to any of these, and there is nothing else: no telemetry, no update check, no fonts or scripts from the internet.`));
    } catch (e) {
      if (isAbort(e) || e.status === 401 || e.status === 403) return;
      mount(outboundSlot, errorState(e, loadOutbound));
    }
  }

  // ------------------------------------------------------------ access & permissions
  // Rows: "Everyone" (the defaults) and one per user. A switch takes effect at once. What everyone
  // has is shown as on, and locked, on each person's row, because personal grants only ever add.
  async function renderAccess() {
    if (!isAdmin()) {
      mount(accessSlot, h('p', { class: 'help' }, 'Only Jellyfin administrators can change who has access to finstats and what they can see.'));
      return;
    }
    let data;
    try { data = await api.get('/permissions', null, { signal: ctx.signal }); }
    catch (e) { if (isAbort(e) || e.status === 401) return; mount(accessSlot, errorState(e, renderAccess)); return; }
    const perms = data.available || [];
    const admins = (data.users || []).filter((u) => u.is_admin);
    const people = (data.users || []).filter((u) => !u.is_admin);
    let defaults = new Set(data.defaults || []);
    const rowsEl = h('tbody');
    const problem = h('div');

    function row({ id, label, sub, granted, save }) {
      const note = h('span', { class: 'saved-note perm-note', 'aria-live': 'polite' });
      let timer;
      const cells = perms.map((pm) => {
        const inherited = id !== null && defaults.has(pm.key);
        const sw = toggle({ checked: inherited || granted.has(pm.key), labelledby: `perm-h-${pm.key} perm-r-${id || 'all'}`,
          onChange: async (next, revert) => {
            mount(problem, '');
            const want = new Set(granted); if (next) want.add(pm.key); else want.delete(pm.key);
            note.replaceChildren(spinner(12));
            try {
              await save([...want]);
              granted = want;
              note.replaceChildren(icon('check', 13), 'Saved');
              clearTimeout(timer); timer = setTimeout(() => note.replaceChildren(), 1800);
              if (id === null) { defaults = want; paint(); } // everyone's rows inherit from this one
            } catch (e) {
              revert(!next); note.replaceChildren();
              mount(problem, inlineError('perm-err', `Couldn’t save: ${e.message}`));
            }
          } });
        if (inherited) { sw.disabled = true; sw.classList.add('is-inherited'); sw.title = 'Everyone has this, so it can’t be taken away from one person'; }
        return h('td', { class: 'perm-cell' }, sw);
      });
      return h('tr', { 'data-pin': id === null ? '' : null }, // "Everyone" stays on top however the people are sorted
        h('th', { scope: 'row', class: 'perm-who', id: `perm-r-${id || 'all'}` }, label, sub ? h('span', { class: 'perm-sub' }, sub) : null),
        cells, h('td', { class: 'perm-saved' }, note));
    }

    function paint() {
      mount(rowsEl,
        row({ id: null, label: h('span', { class: 'perm-name' }, 'Everyone'), sub: 'Applies to every Jellyfin user', granted: new Set(defaults),
          save: (list) => api.put('/permissions/defaults', { permissions: list }) }),
        people.map((u) => row({ id: u.id,
          label: h('span', { class: 'user-cell' }, avatar(u.id, u.name, { size: 24, hasImage: u.has_image }), h('span', { class: 'perm-name' }, u.name)),
          sub: u.is_disabled ? 'Disabled in Jellyfin' : null, granted: new Set(u.permissions || []),
          save: async (list) => { const r = await api.put(`/permissions/users/${u.id}`, { permissions: list }); u.permissions = r.permissions; } })));
    }
    paint();

    mount(accessSlot,
      h('p', { class: 'help perm-intro' }, 'Jellyfin administrators',
        admins.length ? [' (', admins.map((u) => u.name).join(', '), ')'] : null,
        ' always have full access. Everyone else gets what you switch on here. No permission opens other people’s recaps; only administrators can look at those.'),
      dataTable(
        h('table', { class: 'perm-table' },
          h('thead', null, h('tr', null, h('th', { scope: 'col' }, 'Who'),
            perms.map((pm) => h('th', { scope: 'col', id: `perm-h-${pm.key}`, title: pm.description }, pm.label)), h('th', { 'data-nosort': '' }, h('span', { class: 'sr-only' }, 'Status')))),
          rowsEl)),
      problem,
      h('dl', { class: 'perm-legend' }, perms.map((pm) => [h('dt', null, pm.label), h('dd', null, pm.description)])));
  }

  const FIELDS = [
    { key: 'active_interval_s', label: 'While someone is watching, check every', unit: 'seconds', min: 1, max: 60, help: 'How closely a running play is followed: pauses, skips and track changes are recorded to this precision. 1–60.' },
    { key: 'idle_interval_s', label: 'While nothing is playing, check every', unit: 'seconds', min: 1, max: 60, help: 'How soon a new play is noticed. The time before that is not counted, so keep it short. 1–60.' },
    { key: 'sync_interval_h', label: 'Otherwise, re-read the library every', unit: 'hours', min: 1, max: 168, help: 'Only used when finstats is not following Jellyfin’s scan, or the server doesn’t report one. 1–168.' },
    { key: 'merge_window_s', label: 'Treat a restart as the same play within', unit: 'seconds', min: 0, max: 86400, help: 'If the same user resumes the same title on the same device within this window, it counts as one play. 0 turns merging off.' },
    { key: 'group_window_s', label: 'Count it as watching together within', unit: 'seconds', min: 5, max: 600, help: 'Different people who start the same title this close together, and keep watching for a couple of minutes, are counted as a group. Real groups rarely start within 5 seconds: polling and late joiners spread them over up to a minute. 5–600.' },
    { key: 'min_play_s', label: 'Ignore plays shorter than', unit: 'seconds', min: 0, max: 3600, help: 'Short plays stay in the database but are left out of stats. 0 counts everything.' },
  ];

  /** A form of whole-number settings, checked here and saved in one request. */
  function numberForm(FIELDS, errId) {
    const inputs = {};
    const errs = {};
    const rows = FIELDS.map((f) => {
      const input = h('input', { class: 'input input-num mono', type: 'text', inputMode: 'numeric', id: 'f-' + f.key, name: f.key, value: String(settingsData[f.key] ?? ''),
        autocomplete: 'off', 'aria-describedby': `h-${f.key}` });
      inputs[f.key] = input;
      errs[f.key] = h('div');
      return h('div', { class: 'field' },
        h('label', { class: 'setting-label', htmlFor: 'f-' + f.key }, f.label),
        h('div', { class: 'field-input' }, input, h('span', { class: 'unit' }, f.unit)),
        h('p', { class: 'help', id: `h-${f.key}` }, f.help), errs[f.key]);
    });
    const note = h('span', { class: 'saved-note', 'aria-live': 'polite' });
    const formErr = h('div');
    const save = h('button', { type: 'submit', class: 'btn btn-primary' }, 'Save changes');
    let noteTimer;
    const form = h('form', { class: 'form-grid', noValidate: true }, rows, h('div', { class: 'form-actions' }, save, note), formErr);
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      mount(formErr, '');
      const body = {};
      let firstBad = null;
      for (const f of FIELDS) {
        const raw = inputs[f.key].value.trim();
        const n = Number(raw);
        const bad = raw === '' || !/^\d+$/.test(raw) ? `Enter a whole number between ${f.min} and ${num(f.max)}.`
          : n < f.min || n > f.max ? `Must be between ${f.min} and ${num(f.max)}.` : null;
        inputs[f.key].setAttribute('aria-invalid', bad ? 'true' : 'false');
        inputs[f.key].setAttribute('aria-describedby', bad ? `e-${f.key} h-${f.key}` : `h-${f.key}`);
        mount(errs[f.key], bad ? inlineError(`e-${f.key}`, bad) : '');
        if (bad && !firstBad) firstBad = inputs[f.key];
        body[f.key] = n;
      }
      if (firstBad) { firstBad.focus(); return; }
      setBusy(save, true, 'Saving…');
      try {
        settingsData = await api.put('/settings', body);
        for (const f of FIELDS) inputs[f.key].value = String(settingsData[f.key]);
        note.replaceChildren(icon('check', 13), 'Saved');
        clearTimeout(noteTimer); noteTimer = setTimeout(() => note.replaceChildren(), 2000);
      } catch (err) {
        mount(formErr, inlineError(errId, `Couldn’t save: ${err.message}`));
      } finally { setBusy(save, false); }
    });
    return form;
  }

  function renderCollect() {
    const form = numberForm(FIELDS, 'collect-err');
    mount(collectSlot, toggleRow({ key: 'follow_jellyfin_scan', label: 'Follow Jellyfin’s library scan',
      help: 'finstats never starts a scan on Jellyfin. With this on, it re-reads your library only after Jellyfin’s own “Scan Media Library” task has finished, so Jellyfin’s schedule is the only schedule.' }),
      toggleRow({ key: 'live_socket', label: 'Let Jellyfin push what is playing', onSaved: renderConn,
        help: 'New in this version, and off until you turn it on. Instead of asking Jellyfin what is playing every second, finstats keeps one connection open and is told the moment a play starts, pauses or ends — thousands fewer requests a day, and nothing noticed late. It still checks with a plain request once a minute while something is playing, and goes straight back to asking on a timer if the connection drops, so no viewing is ever missed. The two settings below stay as that fallback.' }),
      form);
  }

  // ------------------------------------------------------------ backups (Jellyfin administrators)
  let backupsData = null;
  let backupPending = null;            // {name, action: 'restore' | 'delete'}: waiting for the second click
  let backupErr = null;
  let restoreSettings = true;
  const restoreUpload = { active: false, progress: 0, name: '' };
  const wasRunning = { backup: false, restore: false };

  async function loadBackups() {
    if (!isAdmin()) return;
    try { backupsData = await api.get('/backups', null, { signal: ctx.signal }); renderBackups(); }
    catch (e) { if (isAbort(e) || e.status === 401) return; mount(backupsSlot, errorState(e, loadBackups)); }
  }

  /** The list changes when a backup finishes, and nearly everything changes when a restore does. */
  function watchBackupTasks(tasks) {
    if (!isAdmin()) return;
    for (const id of ['backup', 'restore']) {
      const t = tasks.find((x) => x.id === id);
      const running = !!t && t.state === 'running';
      if (wasRunning[id] && !running) { loadBackups(); if (id === 'restore') loadSettings(); }
      wasRunning[id] = running;
    }
    renderBackups();
  }

  let backupsSig = '';
  function renderBackups() {
    if (!isAdmin() || !backupsData || !settingsData) return;
    const tasks = (tasksData && tasksData.tasks) || [];
    const bk = tasks.find((t) => t.id === 'backup'), rs = tasks.find((t) => t.id === 'restore');
    const busy = (bk && bk.state === 'running') || (rs && rs.state === 'running') || restoreUpload.active;
    const sig = JSON.stringify([backupsData, bk, rs, backupPending, backupErr, restoreSettings, restoreUpload, settingsData.backup_every_d, settingsData.backup_keep]);
    if (sig === backupsSig) return;
    backupsSig = sig;

    const act = async (fn) => { backupErr = null; try { await fn(); } catch (e) { backupErr = e.message; } backupPending = null; setPollInterval(1000); await loadTasks(); await loadBackups(); backupsSig = ''; renderBackups(); };
    const rows = backupsData.backups || [];
    const table = rows.length ? plainTable(h('table', { class: 'table backups' },
      h('thead', null, h('tr', null, h('th', null, 'Made'), h('th', { class: 'r' }, 'Size'), h('th', { 'data-nosort': '' }, h('span', { class: 'sr-only' }, 'Actions')))),
      h('tbody', null, rows.map((b) => {
        const pending = backupPending && backupPending.name === b.name ? backupPending.action : null;
        const ask = (action) => () => { backupPending = { name: b.name, action }; backupsSig = ''; renderBackups(); };
        const cancel = h('button', { type: 'button', class: 'btn btn-sm btn-ghost', onClick: () => { backupPending = null; backupsSig = ''; renderBackups(); } }, 'Cancel');
        const actions = pending === 'delete'
          ? [h('span', { class: 'muted' }, 'Delete this backup?'), h('button', { type: 'button', class: 'btn btn-sm btn-danger', onClick: () => act(() => api.del(`/backups/${b.name}`)) }, 'Delete'), cancel]
          : pending === 'restore'
            ? [h('span', { class: 'muted' }, restoreSettings ? 'Merge its history in and replace settings and permissions?' : 'Merge its history in?'),
              h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: () => act(() => api.post(`/backups/${b.name}/restore?settings=${restoreSettings}`)) }, 'Restore'), cancel]
            : [h('a', { class: 'btn btn-sm', href: `/api/backups/${b.name}`, download: b.name }, icon('upload', 12, 'flip-v'), 'Download'),
              h('button', { type: 'button', class: 'btn btn-sm btn-ghost', disabled: busy, onClick: ask('restore') }, 'Restore'),
              h('button', { type: 'button', class: 'icon-btn', 'aria-label': `Delete the backup from ${dateTime(b.created_at)}`, title: 'Delete', disabled: busy, onClick: ask('delete') }, icon('trash', 14))];
        return h('tr', null,
          h('td', null, h('span', { class: 'when-cell' }, h('time', { dateTime: new Date(b.created_at * 1000).toISOString(), title: b.name }, dateTime(b.created_at)), h('span', { class: 'cell-sub mono' }, relTime(b.created_at)))),
          h('td', { class: 'mono r' }, bytes(b.size_bytes)),
          h('td', null, h('div', { class: 'backup-actions' }, actions)));
      }))))
      : h('p', { class: 'help' }, settingsData.backup_every_d > 0 ? 'No backups yet. The first one is written by itself once there is something to back up, or make one now.' : 'No backups yet, and automatic backups are off.');

    const progress = (t, label) => t && t.state === 'running' ? h('div', { class: 'task-progress' },
      h('div', { class: ['meter meter-wide', t.progress == null && 'is-indeterminate'], role: 'progressbar', 'aria-label': label, 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': t.progress == null ? null : Math.round(t.progress * 100) },
        h('span', { class: 'meter-fill', style: { width: (t.progress == null ? 30 : t.progress * 100) + '%' } })),
      h('span', { class: 'mono task-msg' }, t.message || 'Working…')) : null;
    const restored = rs && rs.state === 'ok' && rs.result ? h('p', { class: 'sev sev-good' }, icon('check', 13),
      `Restored ${num(rs.result.plays_imported)} plays, ${num(rs.result.plays_skipped)} were already here${rs.result.settings_restored ? '; settings and permissions restored' : ''}.`) : null;
    const failed = rs && rs.state === 'error' && rs.error ? inlineError('restore-err', `Restore failed: ${rs.error} Nothing was changed.`) : null;

    const makeNow = h('button', { type: 'button', class: 'btn', disabled: busy, onClick: () => act(() => api.post('/backups')) }, icon('plus', 13), 'Back up now');
    const file = h('input', { type: 'file', class: 'sr-only', id: 'restore-file', accept: '.gz,.jsonl,application/gzip', tabindex: -1 });
    file.addEventListener('change', () => {
      const f = file.files && file.files[0];
      if (!f) return;
      Object.assign(restoreUpload, { active: true, progress: 0, name: f.name }); backupErr = null; backupsSig = ''; renderBackups();
      uploadRaw(`/backups/restore?settings=${restoreSettings}`, f, (p) => { restoreUpload.progress = p; backupsSig = ''; renderBackups(); }).promise
        .catch((e) => { backupErr = e.status === 413 ? 'The server rejected the file as too large. Behind a reverse proxy, raise its upload limit and try again.' : e.message; })
        .finally(() => { restoreUpload.active = false; setPollInterval(1000); loadTasks(); backupsSig = ''; renderBackups(); });
    });
    const keepSettings = h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: restoreSettings, onChange: (e) => { restoreSettings = e.target.checked; backupsSig = ''; renderBackups(); } }),
      h('span', null, 'Also restore settings and permissions'));

    mount(backupsSlot,
      h('div', { class: 'backup-head' },
        h('p', { class: 'help' }, settingsData.backup_every_d > 0
          ? [`A backup is written every ${settingsData.backup_every_d === 1 ? 'day' : num(settingsData.backup_every_d) + ' days'} and the newest ${num(settingsData.backup_keep)} are kept`,
            backupsData.next_at ? [', next ', h('span', { title: dateTime(backupsData.next_at) }, untilText(backupsData.next_at)), '.'] : '.']
          : 'Automatic backups are off.', ' They live in the ', h('span', { class: 'mono' }, 'backups'), ' folder of your data directory.'),
        makeNow),
      progress(bk, 'Backup progress'), bk && bk.state === 'error' && bk.error ? inlineError('backup-err', `Backup failed: ${bk.error}`) : null,
      table,
      backupErr ? inlineError('backups-err', backupErr) : null,
      h('div', { class: 'field' },
        h('div', { class: 'setting-label' }, 'Restore from a file'),
        h('p', { class: 'help' }, 'Moving to a new finstats? Set it up, then restore a downloaded backup here. Restoring merges: plays that are already there are skipped, so it is safe to do twice. A backup holds everyone’s viewing history and IP addresses, but never your Jellyfin API key.'),
        restoreUpload.active
          ? h('div', { class: 'task-progress' }, h('div', { class: 'meter meter-wide', role: 'progressbar', 'aria-label': 'Upload progress', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': Math.round(restoreUpload.progress * 100) },
            h('span', { class: 'meter-fill', style: { width: restoreUpload.progress * 100 + '%' } })), h('span', { class: 'mono task-msg' }, `Uploading ${restoreUpload.name} · ${Math.round(restoreUpload.progress * 100)}%`))
          : h('div', { class: 'backup-restore' }, file, h('label', { class: ['btn', busy && 'is-disabled'], htmlFor: busy ? null : 'restore-file' }, icon('upload', 13), 'Choose a backup file…'), keepSettings),
        progress(rs, 'Restore progress'), restored, failed),
      scheduleForm());
  }

  function scheduleForm() {
    const every = h('input', { class: 'input input-num mono', type: 'text', inputMode: 'numeric', id: 'f-backup-every', value: String(settingsData.backup_every_d), autocomplete: 'off' });
    const keep = h('input', { class: 'input input-num mono', type: 'text', inputMode: 'numeric', id: 'f-backup-keep', value: String(settingsData.backup_keep), autocomplete: 'off' });
    const err = h('div'), note = h('span', { class: 'saved-note', 'aria-live': 'polite' });
    const save = h('button', { type: 'submit', class: 'btn' }, 'Save schedule');
    const form = h('form', { class: 'form-grid', noValidate: true },
      h('div', { class: 'field' }, h('label', { class: 'setting-label', htmlFor: 'f-backup-every' }, 'Back up every'), h('div', { class: 'field-input' }, every, h('span', { class: 'unit' }, 'days')),
        h('p', { class: 'help' }, '7 is a weekly backup. 0 turns automatic backups off. 0–365.')),
      h('div', { class: 'field' }, h('label', { class: 'setting-label', htmlFor: 'f-backup-keep' }, 'Keep the newest'), h('div', { class: 'field-input' }, keep, h('span', { class: 'unit' }, 'backups')),
        h('p', { class: 'help' }, 'Older ones are removed when a new one is written. 1–100.')),
      h('div', { class: 'form-actions' }, save, note), err);
    form.addEventListener('submit', async (e) => {
      e.preventDefault(); mount(err, '');
      const a = Number(every.value.trim()), k = Number(keep.value.trim());
      if (!/^\d+$/.test(every.value.trim()) || a > 365) { mount(err, inlineError('bk-e', 'Days must be a whole number from 0 to 365.')); every.focus(); return; }
      if (!/^\d+$/.test(keep.value.trim()) || k < 1 || k > 100) { mount(err, inlineError('bk-e', 'Keep must be a whole number from 1 to 100.')); keep.focus(); return; }
      setBusy(save, true, 'Saving…');
      try { settingsData = await api.put('/settings', { backup_every_d: a, backup_keep: k }); await loadBackups(); backupsSig = ''; renderBackups(); }
      catch (e2) { mount(err, inlineError('bk-e', `Couldn’t save: ${e2.message}`)); }
      finally { setBusy(save, false); }
    });
    return form;
  }

  // ------------------------------------------------------------ home network
  // Private addresses are local by nature. The household's own public address is local too (a phone on
  // the Wi-Fi reaching Jellyfin through its public name arrives with it), and finstats has to learn that one.
  function renderNetwork() {
    const services = (settingsData.public_ip_services || []).map((u) => String(u).replace(/^https?:\/\//, ''));
    const known = settingsData.known_home_addresses || [];
    const list = known.length
      ? h('ul', { class: 'home-ips' }, known.map((a) => h('li', null, h('span', { class: 'mono' }, a.ip),
        h('span', { class: 'muted' }, a.source === 'manual' ? 'added by you' : ['found automatically · last seen ', h('span', { title: dateTime(a.last_seen) }, relTime(a.last_seen))]))))
      : h('p', { class: 'help' }, settingsData.public_ip_lookup ? 'No public address learned yet. finstats looks one up within a few minutes of starting.' : 'None. Private addresses (192.168.x.x, 10.x.x.x and the like) always count as local.');

    const input = h('textarea', { class: 'input home-input mono', id: 'f-home', rows: 2, spellcheck: false, autocomplete: 'off', 'aria-describedby': 'h-home',
      placeholder: '203.0.113.7' }, (settingsData.home_addresses || []).join('\n'));
    const note = h('span', { class: 'saved-note', 'aria-live': 'polite' });
    const err = h('div');
    const save = h('button', { type: 'submit', class: 'btn' }, 'Save addresses');
    const form = h('form', { class: 'form-grid', noValidate: true },
      h('div', { class: 'field' }, h('label', { class: 'setting-label', htmlFor: 'f-home' }, 'Other addresses that count as home'), input,
        h('p', { class: 'help', id: 'h-home' }, 'One IP address per line: an earlier public address of yours, a second home, a VPN exit. Every play in your history is sorted into local and remote again when you save.'), err),
      h('div', { class: 'form-actions' }, save, note));
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      mount(err, '');
      const home_addresses = input.value.split(/[\s,;]+/).map((x) => x.trim()).filter(Boolean);
      setBusy(save, true, 'Saving…');
      try {
        settingsData = await api.put('/settings', { home_addresses });
        renderNetwork();
      } catch (e2) {
        input.setAttribute('aria-invalid', 'true');
        mount(err, inlineError('home-err', `Couldn’t save: ${e2.message}`));
      } finally { setBusy(save, false); }
    });

    // The only thing that ever asks again, because a person pressed it.
    const lookupErr = h('div');
    const lookup = h('button', { type: 'button', class: 'btn' }, 'Look up now');
    lookup.addEventListener('click', async () => {
      mount(lookupErr, '');
      setBusy(lookup, true, 'Asking\u2026');
      try {
        settingsData = await api.post('/settings/public-ip');
        renderNetwork();
      } catch (e) {
        mount(lookupErr, inlineError('lookup-err', `Couldn\u2019t look it up: ${e.message}`));
      } finally { setBusy(lookup, false); }
    });

    mount(networkSlot,
      toggleRow({ key: 'public_ip_lookup', label: 'Recognise my own public address', onSaved: renderNetwork,
        help: `A device at home that reaches Jellyfin through its public name shows up with your household’s public IP, which would otherwise look remote. With this on, finstats asks a public “what is my IP” service (${services.join(', ') || 'none configured'}) — once, when it first needs to know, and never again on its own. Plays from that address then count as local, and earlier addresses are remembered, since they change. The request contains nothing about you or your server. With this and the geolocation download under Security both off, finstats makes no outside requests at all.` }),
      h('div', { class: 'field' },
        h('div', { class: 'setting-label' }, 'Known home addresses'), list,
        settingsData.public_ip_lookup ? h('div', { class: 'form-actions' }, lookup, h('span', { class: 'help' }, 'If your address has changed, ask again.')) : null,
        lookupErr),
      form);
  }

  // ------------------------------------------------------------ security (geolocation)
  const TRAVEL_FIELDS = [
    { key: 'travel_speed_kmh', label: 'Impossible travel is faster than', unit: 'km/h', min: 100, max: 5000, help: 'Two sightings of one person that would need more than this speed raise an alert. 900 is a little above an airliner. 100–5,000.' },
    { key: 'travel_min_km', label: 'Only between places at least', unit: 'km apart', min: 50, max: 5000, help: 'City databases are often a few hundred kilometres off, and a phone on mobile data is frequently “in” the capital. Closer places than this never raise an alert. 50–5,000.' },
  ];
  let geoWasRunning = false, geoSig = 'null';

  function renderSecurity() {
    if (!settingsData) return;
    const g = settingsData.geoip || {}, dbInfo = g.database;
    const t = ((tasksData && tasksData.tasks) || []).find((x) => x.id === 'geoip');
    const running = !!t && t.state === 'running';
    const status = dbInfo
      ? h('p', { class: 'help' }, h('strong', null, dbInfo.kind), `, built ${dateTime(dbInfo.built_at).split(',')[0]}`, dbInfo.file ? [' · ', h('span', { class: 'mono' }, dbInfo.file)] : null)
      : h('p', { class: 'help' }, 'None yet. The Security page stays empty until there is one.');
    const err = h('div');
    const get = h('button', { type: 'button', class: 'btn', disabled: running || g.from_env }, icon('upload', 13, 'flip-v'), running ? (t.message || 'Downloading…') : dbInfo ? 'Download the newest now' : 'Download now (about 60 MB)');
    get.addEventListener('click', async () => {
      mount(err, '');
      setBusy(get, true, 'Starting…');
      try { await api.post('/security/database'); setPollInterval(1000); await loadTasks(); }
      catch (e) { setBusy(get, false); mount(err, inlineError('geoip-err', e.message)); }
    });
    if (t && t.state === 'error') mount(err, inlineError('geoip-err', t.error || 'The download failed.'));
    mount(securitySlot,
      h('div', { class: 'field' }, h('div', { class: 'setting-label' }, 'Geolocation database'), status,
        h('p', { class: 'help' }, g.from_env ? 'The file is set with FINSTATS_GEOIP_DB; replace that file to update it.'
          : ['Addresses are looked up in a file on this machine, never over the network. finstats uses the newest ', h('span', { class: 'mono' }, '.mmdb'), ' city database in ', h('span', { class: 'mono' }, g.folder || 'the geoip folder'),
            ': DB-IP’s free one, MaxMind’s GeoLite2-City, or any other in that format.']),
        h('div', { class: 'form-actions' }, get), err),
      g.from_env ? null : toggleRow({ key: 'geoip_download', label: 'Keep the database up to date', onSaved: renderSecurity,
        help: 'Downloads DB-IP’s free “IP to City Lite” file (about 60 MB, licensed CC BY 4.0) from download.db-ip.com now and once a month. The request is a plain file download and contains nothing about you or your server. Off, finstats only uses a file you put there yourself.' }),
      numberForm(TRAVEL_FIELDS, 'travel-err'));
  }

  // ------------------------------------------------------------ tasks
  const runBusy = new Set();
  const taskErr = {};
  let setPollInterval = () => {};
  let sawImportRunning = false;

  async function loadTasks() {
    try {
      tasksData = await api.get('/tasks', null, { signal: ctx.signal });
    } catch (e) {
      if (isAbort(e) || e.status === 401) return;
      if (!tasksData) mount(tasksSlot, errorState(e, loadTasks));
      return;
    }
    const tasks = tasksData.tasks || [];
    const anyRunning = tasks.some((t) => t.state === 'running');
    const imp = tasks.find((t) => t.id === 'import');
    if (imp && imp.state === 'running') sawImportRunning = true;
    setPollInterval(imp && imp.state === 'running' || upload.doneAt && Date.now() - upload.doneAt < 15000 ? 1000 : anyRunning ? 2000 : 10000);
    renderTasks(tasks); renderConn(); renderDb(); renderImport(); watchBackupTasks(tasks);
    const geo = tasks.find((x) => x.id === 'geoip'), geoRunning = !!geo && geo.state === 'running';
    // Only when something changed: a re-render would wipe a number somebody is typing.
    const geoNow = JSON.stringify(geo ? [geo.state, geo.message, geo.error] : null);
    if (geoWasRunning && !geoRunning) loadSettings(); else if (geoNow !== geoSig) renderSecurity();
    geoWasRunning = geoRunning; geoSig = geoNow;
  }

  let tasksSig = '';
  function renderTasks(tasks, force = false) {
    // A task that reads from a service nobody has connected would only ever say so.
    const feat = (state.user && state.user.features) || {};
    tasks = tasks.filter((t) => !SERVICE_TASKS[t.id] || feat[SERVICE_TASKS[t.id]]);
    const sig = JSON.stringify([tasks, [...runBusy], taskErr]);
    if (!force && sig === tasksSig) return; // don't rebuild (and drop focus) when nothing changed
    tasksSig = sig;
    mount(tasksSlot, h('ul', { class: 'tasks' }, tasks.map((t) => {
      const [name, desc] = TASK_LABEL[t.id] || [humanize(String(t.id || 'task').replace(/^sync_/, 'Sync ')), ''];
      const running = t.state === 'running' || runBusy.has(t.id);
      const state = t.state === 'running' ? h('span', { class: 'sev sev-run' }, spinner(12), 'Running')
        : t.state === 'ok' ? h('span', { class: 'sev sev-good' }, icon('check', 13), 'Finished', t.finished_at ? h('span', { class: 'muted mono', title: dateTime(t.finished_at) }, ' ' + relTime(t.finished_at)) : null)
        : t.state === 'error' ? h('span', { class: 'sev sev-critical' }, icon('alert', 13), 'Failed', t.finished_at ? h('span', { class: 'muted mono' }, ' ' + relTime(t.finished_at)) : null)
        : h('span', { class: 'muted' }, 'Hasn’t run yet');
      let btn = null;
      if (!OWN_CARD.has(t.id)) {
        btn = h('button', { type: 'button', class: 'btn btn-sm' }, icon('play', 12), 'Run now');
        if (running) { btn.disabled = true; btn.replaceChildren(spinner(12), h('span', null, 'Running…')); }
        btn.addEventListener('click', async () => {
          runBusy.add(t.id); delete taskErr[t.id]; renderTasks(tasks);
          try { await api.post(`/tasks/${t.id}/run`); }
          catch (e) { if (e.status !== 409) taskErr[t.id] = e.message; }
          runBusy.delete(t.id);
          setPollInterval(2000);
          loadTasks();
        });
      }
      return h('li', { class: 'task' },
        h('div', { class: 'task-main' }, h('div', { class: 'task-name' }, name), h('div', { class: 'help' }, desc),
          t.state === 'running' ? h('div', { class: 'task-progress' },
            h('div', { class: ['meter meter-wide', t.progress == null && 'is-indeterminate'], role: 'progressbar', 'aria-label': name + ' progress', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': t.progress == null ? null : Math.round(t.progress * 100) },
              h('span', { class: 'meter-fill', style: { width: (t.progress == null ? 30 : t.progress * 100) + '%' } })),
            h('span', { class: 'mono task-msg' }, t.message || 'Working…')) : t.message && t.state !== 'idle' ? h('div', { class: 'mono task-msg' }, t.message) : null,
          t.state === 'error' && t.error ? inlineError('task-err-' + t.id, t.error) : null,
          taskErr[t.id] ? inlineError('task-run-err-' + t.id, `Couldn’t start: ${taskErr[t.id]}`) : null),
        h('div', { class: 'task-side' }, state, btn));
    })));
  }

  function renderDb() {
    const d = tasksData && tasksData.db;
    if (!d) return;
    mount(dbSlot, facts([
      ['Size on disk', bytes(d.size_bytes), { mono: true }],
      ['Plays', num(d.plays), { mono: true }],
      ['Library items', num(d.items), { mono: true }],
      ['Oldest play', d.oldest_play_at ? dateTime(d.oldest_play_at) : '–', { mono: true }],
    ]));
  }

  // ------------------------------------------------------------ import
  let localErr = null;
  const fileInput = h('input', { type: 'file', accept: '.jsonl,.json,application/json', class: 'sr-only', id: 'import-file', tabIndex: -1 });
  fileInput.addEventListener('change', () => { if (fileInput.files[0]) pick(fileInput.files[0]); fileInput.value = ''; });

  function pick(file) {
    localErr = null;
    if (!/\.(jsonl|json)$/i.test(file.name)) localErr = `“${file.name}” isn’t a Jellystat backup. Choose the .jsonl file you downloaded from Jellystat’s Backups page.`;
    else if (!file.size) localErr = 'That file is empty. Download the backup from Jellystat again.';
    if (localErr) { renderImport(); return; }
    sawImportRunning = false;
    startUpload(file);
    setPollInterval(1000);
  }

  const RESULT_ROWS = [['plays_imported', 'Plays imported'], ['plays_skipped', 'Duplicates skipped'], ['users', 'Users'], ['libraries', 'Libraries'],
    ['items', 'Movies, series and tracks'], ['seasons', 'Seasons'], ['episodes', 'Episodes'], ['item_info', 'File details']];

  let importSig = null;
  function renderImport() {
    const imp = tasksData && (tasksData.tasks || []).find((t) => t.id === 'import');
    const running = imp && imp.state === 'running';
    const justUploaded = upload.doneAt && Date.now() - upload.doneAt < 15000 && !sawImportRunning && !(imp && imp.state === 'error');
    const sig = JSON.stringify([imp, running, !!justUploaded, upload.active, upload.loaded, upload.error, localErr]);
    if (sig === importSig) return; // keep the drop zone (and its focus) stable between polls
    importSig = sig;

    let stage;
    if (upload.active) {
      stage = h('div', { class: 'import-stage', role: 'status' },
        h('div', { class: 'import-stage-title' }, `Uploading ${upload.fileName}`),
        h('div', { class: 'meter meter-wide', role: 'progressbar', 'aria-label': 'Upload progress', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': Math.round(upload.progress * 100) },
          h('span', { class: 'meter-fill', style: { width: upload.progress * 100 + '%' } })),
        h('div', { class: 'import-stage-row' }, h('span', { class: 'mono' }, `${Math.round(upload.progress * 100)}% · ${bytes(upload.loaded)} of ${bytes(upload.total)}`),
          h('button', { type: 'button', class: 'btn btn-sm', onClick: () => upload.handle && upload.handle.abort() }, 'Cancel upload')),
        h('p', { class: 'help' }, 'Keep this tab open until the upload finishes. You can browse other finstats pages meanwhile.'));
    } else if (running || justUploaded) {
      stage = h('div', { class: 'import-stage', role: 'status' },
        h('div', { class: 'import-stage-title' }, spinner(14), running ? 'Importing your history' : 'Upload complete'),
        h('div', { class: ['meter meter-wide', !(running && imp.progress != null) && 'is-indeterminate'], role: 'progressbar', 'aria-label': 'Import progress', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': running && imp.progress != null ? Math.round(imp.progress * 100) : null },
          h('span', { class: 'meter-fill', style: { width: (running && imp.progress != null ? imp.progress * 100 : 30) + '%' } })),
        h('div', { class: 'mono import-msg', 'aria-live': 'polite' }, running ? imp.message || 'Reading the backup…' : 'Starting the import…'),
        h('p', { class: 'help' }, 'This runs on the server. It’s safe to leave this page.'));
    } else {
      const drop = h('label', { class: 'dropzone', htmlFor: 'import-file', tabindex: 0, role: 'button', 'aria-label': 'Choose a Jellystat backup file' },
        icon('upload', 20), h('span', { class: 'dropzone-title' }, 'Drop your Jellystat backup here'),
        h('span', { class: 'dropzone-sub' }, 'or click to choose the .jsonl file — large backups are fine'));
      drop.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); fileInput.click(); } });
      drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('is-over'); });
      drop.addEventListener('dragleave', () => drop.classList.remove('is-over'));
      drop.addEventListener('drop', (e) => { e.preventDefault(); drop.classList.remove('is-over'); const f = e.dataTransfer.files[0]; if (f) pick(f); });

      let outcome = null;
      if (imp && imp.state === 'ok' && imp.result) {
        outcome = h('div', { class: 'import-result' },
          h('div', { class: 'sev sev-good' }, icon('check', 14), 'Import finished', imp.finished_at ? h('span', { class: 'muted mono', title: dateTime(imp.finished_at) }, ' ' + relTime(imp.finished_at)) : null),
          imp.message ? h('p', { class: 'help' }, imp.message) : null,
          h('table', { class: 'table table-dense result-table' }, h('tbody', null, RESULT_ROWS.filter(([k]) => imp.result[k] != null).map(([k, label]) =>
            h('tr', null, h('th', { scope: 'row' }, label), h('td', { class: 'mono r' }, num(imp.result[k])))))),
          h('a', { class: 'btn btn-sm', href: '/' }, 'See your stats', icon('chevronRight', 14)));
      } else if (imp && imp.state === 'error') {
        outcome = h('div', { class: 'import-result' }, inlineError('import-task-err', `The import failed: ${imp.error || imp.message || 'unknown error'}`),
          h('p', { class: 'help' }, 'Nothing was half-imported. Check that the file is an unmodified Jellystat backup, then upload it again.'));
      }
      stage = [drop,
        localErr ? inlineError('import-local-err', localErr) : null,
        upload.error ? inlineError('import-upload-err', `Upload failed: ${upload.error}`) : null,
        outcome];
    }

    mount(importSlot,
      h('div', { class: 'import-cols' },
        h('div', null, h('h3', { class: 'section-label' }, 'Export from Jellystat'),
          h('ol', { class: 'steps' },
            h('li', null, 'Open your Jellystat instance.'),
            h('li', null, 'Go to ', h('strong', null, 'Settings'), ' and select the ', h('strong', null, 'Backup'), ' tab.'),
            h('li', null, 'Select only ', h('strong', null, 'Activity'), ' — it turns purple when selected.'),
            h('li', null, 'Under settings, click ', h('strong', null, 'Settings'), '.'),
            h('li', null, 'Scroll all the way to the end and start a backup.'),
            h('li', null, 'Go back to ', h('strong', null, 'Backups'), '.'),
            h('li', null, 'Once the new backup shows up, open its ', h('strong', null, 'Actions'), ' menu and click ', h('strong', null, 'Download'), '.'),
            h('li', null, 'Upload the file here.'))),
        h('div', null, h('h3', { class: 'section-label' }, 'Upload the backup'), fileInput, stage,
          h('p', { class: 'help import-note' }, icon('info', 13), ' Importing the same backup again is safe — plays that are already here are skipped. Backups that also contain libraries and users work too.'))));
  }

  const unsub = () => uploadSubs.delete(onUpload);
  const onUpload = () => { renderImport(); if (!upload.active && upload.doneAt) loadTasks(); };
  uploadSubs.add(onUpload);
  ctx.onCleanup(unsub);

  renderImport();
  loadSettings();
  loadTasks();
  setPollInterval = ctx.every(loadTasks, 10000);
}
