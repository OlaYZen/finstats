// Settings → Connections: Sonarr, Radarr, Seerr and the torrent clients. Jellyfin administrators only.
// A connection is tested before it is saved (the server does that too), and a key or password is
// write-only: the API says whether one is stored, never what it is.

import { h, icon, mount, relTime, dateTime } from './dom.js';
import { api, isAbort } from './api.js';
import { state } from './state.js';
import { setBusy, inlineError, formField, errorState, sk } from './components.js';

const SECRET_LABEL = { api_key: 'API key', login: 'Password', password: 'Password' };
const SECRET_HELP = {
  sonarr: 'Sonarr → Settings → General → Security → API Key.',
  radarr: 'Radarr → Settings → General → Security → API Key.',
  seerr: 'Seerr → Settings → General → API Key.',
  qbittorrent: 'The user name and password of qBittorrent’s Web UI (Tools → Options → Web UI).',
  transmission: 'Only if Transmission asks for one (Remote → “Use authentication”). Leave both empty otherwise.',
  deluge: 'The password of Deluge’s web page. It must be connected to its daemon (Connection Manager).',
};

/** A self-managing card body. `ctx` is the page context (signal, every). */
export function connectionsPanel(ctx) {
  const root = h('div', { class: 'net-stack' }, sk.rows(2));
  let data = null;          // { services, kinds }
  let editing = null;       // null | 'new' | service id
  let removing = null;      // service id waiting for the second click
  let lastSig = '';
  let rowErr = null;        // {id, text}: a removal that failed

  async function load({ quiet = false } = {}) {
    try {
      data = await api.get('/services', null, { signal: ctx.signal });
    } catch (e) {
      if (isAbort(e) || e.status === 401) return;
      if (!quiet) mount(root, errorState(e, load));
      return;
    }
    // While a form is open, a background refresh must not wipe what is being typed.
    const sig = JSON.stringify(data);
    if (editing !== null && quiet) return;
    if (sig === lastSig && quiet) return;
    lastSig = sig;
    render();
  }

  function statusOf(s) {
    if (!s.enabled) return h('span', { class: 'sev sev-info' }, icon('minus', 13), 'Switched off');
    if (s.last_error) return h('span', { class: 'sev sev-critical' }, icon('alert', 13), h('span', null, 'Not answering: ', s.last_error));
    if (s.last_ok_at) return h('span', { class: 'sev sev-good' }, icon('check', 13),
      h('span', null, 'Connected', s.version ? h('span', { class: 'mono' }, ` · ${s.version}`) : null, ' · checked ', h('span', { title: dateTime(s.last_ok_at) }, relTime(s.last_ok_at))));
    return h('span', { class: 'sev sev-info' }, icon('clock', 13), 'Not checked yet');
  }

  function row(s) {
    const act = async (fn) => { try { data = await fn(); } catch (e) { rowErr = { id: s.id, text: e.message }; } removing = null; lastSig = ''; render(); };
    const actions = removing === s.id
      ? [h('span', { class: 'muted' }, 'Remove it, and everything finstats read from it?'),
        h('button', { type: 'button', class: 'btn btn-sm btn-danger', onClick: (e) => { setBusy(e.currentTarget, true, 'Removing…'); act(() => api.del(`/services/${s.id}`)); } }, 'Remove'),
        h('button', { type: 'button', class: 'btn btn-sm btn-ghost', onClick: () => { removing = null; render(); } }, 'Cancel')]
      : [h('button', { type: 'button', class: 'btn btn-sm', onClick: () => { editing = s.id; removing = null; render(); } }, 'Edit'),
        h('button', { type: 'button', class: 'btn btn-sm btn-ghost btn-danger-text', onClick: () => { removing = s.id; render(); } }, 'Remove…')];
    return h('li', { class: 'conn-row' },
      h('div', { class: 'conn-main' },
        h('div', { class: 'conn-name' }, h('strong', null, s.name), s.name !== s.label ? h('span', { class: 'chip' }, s.label) : null),
        h('div', { class: 'conn-url mono' }, s.url),
        h('div', { class: 'conn-status' }, statusOf(s)),
        rowErr && rowErr.id === s.id ? inlineError(`conn-err-${s.id}`, rowErr.text) : null),
      h('div', { class: 'conn-actions' }, actions));
  }
  function form(existing) {
    const kinds = data.kinds;
    let kind = existing ? kinds.find((k) => k.key === existing.kind) : kinds[0];
    const kindSel = h('select', { class: 'input', id: 'conn-kind', 'aria-describedby': 'conn-kind-help' }, kinds.map((k) => h('option', { value: k.key }, k.label)));
    const kindHelp = h('p', { class: 'help', id: 'conn-kind-help' });
    const name = formField({ id: 'conn-name', label: 'Name (optional)', autocomplete: 'off', help: 'Shown in finstats. Useful with two of a kind: “Radarr 4K”.' });
    const url = formField({ id: 'conn-url', label: 'Address', autocomplete: 'off', inputMode: 'url' });
    const user = formField({ id: 'conn-user', label: 'User name', autocomplete: 'off' });
    const secret = formField({ id: 'conn-secret', label: 'API key', type: 'password', autocomplete: 'new-password', help: ' ' });
    const certs = h('input', { type: 'checkbox', id: 'conn-certs', 'aria-describedby': 'conn-certs-help' });
    const enabled = h('input', { type: 'checkbox', id: 'conn-enabled' });
    const result = h('div', { 'aria-live': 'polite' });
    const formErr = h('div');
    const testBtn = h('button', { type: 'button', class: 'btn' }, 'Test connection');
    const saveBtn = h('button', { type: 'submit', class: 'btn btn-primary' }, existing ? 'Save' : 'Test and save');

    function paintKind() {
      kindHelp.textContent = kind.what;
      url.input.placeholder = kind.example;
      user.el.hidden = kind.auth !== 'login';
      secret.el.querySelector('label').textContent = SECRET_LABEL[kind.auth];
      secret.el.querySelector('.help').textContent = (existing && existing.has_secret ? 'Leave empty to keep the stored one. ' : '') + (SECRET_HELP[kind.key] || '');
      secret.input.placeholder = existing && existing.has_secret ? 'Unchanged' : '';
    }
    if (existing) {
      kindSel.value = existing.kind; kindSel.disabled = true;
      name.input.value = existing.name; url.input.value = existing.url; user.input.value = existing.username || '';
      certs.checked = !!existing.accept_invalid_certs; enabled.checked = !!existing.enabled;
    } else enabled.checked = true;
    kindSel.addEventListener('change', () => { kind = kinds.find((k) => k.key === kindSel.value); mount(result, ''); paintKind(); });
    for (const f of [url, user, secret]) f.input.addEventListener('input', () => { f.setError(''); mount(result, ''); });
    paintKind();

    const body = () => {
      const b = { kind: kind.key, name: name.input.value.trim(), url: url.input.value.trim(), accept_invalid_certs: certs.checked, enabled: enabled.checked };
      if (kind.auth === 'login') b.username = user.input.value.trim();
      if (secret.input.value) b.secret = secret.input.value;
      if (existing) b.id = existing.id;
      return b;
    };
    function valid() {
      let ok = true;
      if (!url.input.value.trim()) { url.setError(`Enter the address of ${kind.label}, like ${kind.example}.`); ok = false; }
      if (kind.auth === 'api_key' && !secret.input.value && !(existing && existing.has_secret)) { secret.setError('Enter the API key.'); ok = false; }
      if (!ok) (url.input.getAttribute('aria-invalid') === 'true' ? url.input : secret.input).focus();
      return ok;
    }
    // The server's sentence names what is wrong; put it next to the field it is about.
    function place(err) {
      mount(result, '');
      const text = err.message || 'Something went wrong.';
      if (/API key|password|user name|blocked this address/i.test(text)) { secret.setError(text); secret.input.focus(); }
      else if (err.status === 400 || err.status === 502) { url.setError(text); url.input.focus(); }
      else mount(formErr, inlineError('conn-form-err', text));
    }
    testBtn.addEventListener('click', async () => {
      mount(formErr, '');
      if (!valid()) return;
      setBusy(testBtn, true, 'Testing…');
      try {
        const r = await api.post('/services/test', body());
        mount(result, h('p', { class: 'sev sev-good test-ok' }, icon('check', 14), h('span', null, 'Connected to ', h('strong', null, r.app || kind.label), r.version ? h('span', { class: 'mono' }, ` · ${r.version}`) : null)));
      } catch (e) { place(e); } finally { setBusy(testBtn, false); }
    });
    const el = h('form', { class: 'conn-form', noValidate: true },
      h('h3', { class: 'conn-form-title' }, existing ? `Edit ${existing.name}` : 'Add a connection'),
      h('div', { class: 'form-grid' },
        h('div', { class: 'field' }, h('label', { class: 'field-label', htmlFor: 'conn-kind' }, 'Service'), kindSel, kindHelp),
        name.el, url.el, user.el, secret.el,
        h('div', { class: 'field' },
          h('label', { class: 'check' }, certs, 'Accept a self-signed certificate'),
          h('p', { class: 'help', id: 'conn-certs-help' }, 'Only for an https:// address whose certificate is your own. finstats then does not verify who answers at this address. Leave it off otherwise.'),
          existing ? h('label', { class: 'check' }, enabled, 'Switched on') : null)),
      result, formErr,
      h('div', { class: 'form-actions' }, saveBtn, testBtn, h('button', { type: 'button', class: 'btn btn-ghost', onClick: () => { editing = null; render(); } }, 'Cancel')));
    el.addEventListener('submit', async (e) => {
      e.preventDefault();
      mount(formErr, '');
      if (!valid()) return;
      setBusy(saveBtn, true, 'Testing…');
      try {
        data = existing ? await api.put(`/services/${existing.id}`, body()) : await api.post('/services', body());
        editing = null; lastSig = '';
        // Which pages exist depends on what is connected: ask again who we are and what there is.
        try { state.user = (await api.get('/auth/me')).user; } catch { /* the next navigation will */ }
        render();
      } catch (err) { setBusy(saveBtn, false); place(err); }
    });
    return el;
  }

  function render() {
    if (!data) return;
    const list = data.services.length
      ? h('ul', { class: 'conn-list' }, data.services.map(row))
      : h('p', { class: 'help' }, 'Nothing connected yet. With Sonarr and Radarr, finstats knows what is coming; with Seerr, who asked for what; with a torrent client, what is arriving right now.');
    const existing = typeof editing === 'number' ? data.services.find((s) => s.id === editing) : null;
    mount(root,
      h('p', { class: 'help' }, 'finstats only reads from these services: it never approves a request, starts a search or touches a download. Keys and passwords are stored in finstats’ own database, are never shown again and are never part of a backup.'),
      list,
      editing === null
        ? h('div', { class: 'form-actions' }, h('button', { type: 'button', class: 'btn', onClick: () => { editing = 'new'; removing = null; render(); } }, icon('plus', 14), 'Add a connection'))
        : form(existing));
    if (editing !== null) { const first = root.querySelector(existing ? '#conn-name' : '#conn-kind'); if (first) first.focus(); }
  }

  load();
  ctx.every(() => load({ quiet: true }), 30000, { visibleOnly: true });
  return root;
}
