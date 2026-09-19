import { h, icon, num, bytes, relTime, dateTime, mount, humanize } from '../dom.js';
import { api, isAbort, uploadRaw } from '../api.js';
import { isAdmin } from '../state.js';
import { pageHeader, card, sk, toggle, setBusy, inlineError, errorState, facts, spinner, avatar } from '../components.js';
import { dataTable } from '../tables.js';

const TASK_LABEL = {
  sync_users: ['Sync users', 'Names, roles and last-seen times from Jellyfin'],
  sync_libraries: ['Read libraries and items', 'Copies titles and file details from Jellyfin. Read-only: it never starts a scan on Jellyfin'],
  sync_events: ['Sync server log', 'Jellyfin’s activity log: sign-ins, failed logins, tasks'],
  sync_server: ['Server details', 'Version, storage, plugins, scheduled tasks and devices'],
  sync_userdata: ['Watched & favourites', 'Per-user played flags and favourites from Jellyfin'],
  import: ['Jellystat import', 'Runs when you upload a backup below'],
};

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
  const tasksSlot = h('div', null, sk.rows(3));
  const importSlot = h('div');
  const dbSlot = h('div', null, sk.rows(1));

  ctx.root.append(pageHeader('Settings', 'Connection, access, collection and data'),
    h('div', { class: 'stack settings' },
      card({ title: 'Jellyfin connection', body: connSlot }),
      card({ title: 'Access', sub: 'Who can use finstats and what they can see', body: accessSlot }),
      card({ title: 'Collection', sub: 'How finstats gathers data from Jellyfin', body: collectSlot }),
      card({ title: 'Tasks', body: tasksSlot }),
      card({ title: 'Import from Jellystat', sub: 'Bring your playback history with you', body: importSlot, id: 'import' }),
      card({ title: 'Database', body: dbSlot })));

  // ------------------------------------------------------------ settings
  let settingsData = null;
  async function loadSettings() {
    try {
      settingsData = await api.get('/settings', null, { signal: ctx.signal });
      renderAccess(); renderCollect(); renderConn();
    } catch (e) {
      if (isAbort(e) || e.status === 401) return;
      mount(accessSlot, errorState(e, loadSettings)); mount(collectSlot, ''); mount(connSlot, '');
    }
  }

  let tasksData = null;
  function renderConn() {
    if (!settingsData) return;
    const c = tasksData && tasksData.collector;
    const status = !c ? h('span', { class: 'muted' }, 'Checking…')
      : c.connected ? h('span', { class: 'sev sev-good' }, icon('check', 13), `Connected · ${num(c.active_sessions)} active ${c.active_sessions === 1 ? 'session' : 'sessions'}`)
      : h('span', { class: 'sev sev-critical' }, icon('alert', 13), 'Not connected' + (c.error ? ` — ${c.error}` : ''));
    mount(connSlot, facts([
      ['Server', settingsData.server_name],
      ['Address', settingsData.jellyfin_url, { mono: true }],
      ['Jellyfin version', settingsData.server_version, { mono: true }],
      ['Collector', status],
      c && c.last_poll_at ? ['Last checked', h('span', { title: dateTime(c.last_poll_at) }, relTime(c.last_poll_at)), { mono: true }] : null,
    ]), h('p', { class: 'help' }, 'finstats talks to Jellyfin with its own API key, created during setup. To point finstats at another server, start it with a fresh data directory.'));
  }

  /** An immediate-effect setting: a switch that saves on change and confirms next to itself. */
  function toggleRow({ key, label, help }) {
    const note = h('span', { class: 'saved-note', 'aria-live': 'polite' });
    const err = h('div');
    let noteTimer;
    const sw = toggle({ checked: !!settingsData[key], labelledby: `${key}-label`, describedby: `${key}-help`,
      onChange: async (next, revert) => {
        mount(err, ''); note.replaceChildren(spinner(12));
        try {
          settingsData = await api.put('/settings', { [key]: next });
          note.replaceChildren(icon('check', 13), 'Saved');
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
    { key: 'poll_interval_s', label: 'Check for playback every', unit: 'seconds', min: 2, max: 60, help: 'How often finstats asks Jellyfin what’s playing. 2–60.' },
    { key: 'sync_interval_h', label: 'Otherwise, re-read the library every', unit: 'hours', min: 1, max: 168, help: 'Only used when finstats is not following Jellyfin’s scan, or the server doesn’t report one. 1–168.' },
    { key: 'merge_window_s', label: 'Treat a restart as the same play within', unit: 'seconds', min: 0, max: 86400, help: 'If the same user resumes the same title on the same device within this window, it counts as one play. 0 turns merging off.' },
    { key: 'group_window_s', label: 'Count it as watching together within', unit: 'seconds', min: 5, max: 600, help: 'Different people who start the same title this close together, and keep watching for a couple of minutes, are counted as a group. Real groups rarely start within 5 seconds: polling and late joiners spread them over up to a minute. 5–600.' },
    { key: 'min_play_s', label: 'Ignore plays shorter than', unit: 'seconds', min: 0, max: 3600, help: 'Short plays stay in the database but are left out of stats. 0 counts everything.' },
  ];

  function renderCollect() {
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
        mount(formErr, inlineError('collect-err', `Couldn’t save: ${err.message}`));
      } finally { setBusy(save, false); }
    });
    mount(collectSlot, toggleRow({ key: 'follow_jellyfin_scan', label: 'Follow Jellyfin’s library scan',
      help: 'finstats never starts a scan on Jellyfin. With this on, it re-reads your library only after Jellyfin’s own “Scan Media Library” task has finished, so Jellyfin’s schedule is the only schedule.' }), form);
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
    renderTasks(tasks); renderConn(); renderDb(); renderImport();
  }

  let tasksSig = '';
  function renderTasks(tasks, force = false) {
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
      if (t.id !== 'import') {
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
