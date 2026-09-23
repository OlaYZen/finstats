// Settings → Notifications: where what finstats finds is sent. A destination belongs either to the
// server (Jellyfin administrators) or to one person, and a person's own destination is only ever sent
// what they may already see in the app — the server decides that, this only draws it.
//
// The address is write-only, like a key: a Discord webhook URL carries its own token, so the API
// answers the host and never the URL. Editing without re-typing it keeps the stored one.

import { h, icon, mount, relTime, dateTime } from './dom.js';
import { api, isAbort } from './api.js';
import { isAdmin, can } from './state.js';
import { setBusy, inlineError, formField, errorState, sk } from './components.js';
import { dataTable } from './tables.js';

const SECRET_HELP = {
  webhook: 'Sent as “Authorization: Bearer …”. Leave empty if your receiver needs no header.',
  ntfy: 'Only for a protected topic or your own ntfy with access control.',
  gotify: 'Gotify → Apps → create an application → its token.',
};

const STATE_LABEL = { sent: 'Sent', queued: 'Waiting', failed: 'Given up' };

export function notificationsPanel(ctx) {
  const root = h('div', { class: 'net-stack' }, sk.rows(2));
  let data = null;         // { targets, catalogue, public_url, can_add_server, max_own }
  let history = null;      // { events }
  let editing = null;      // null | 'new' | id
  let removing = null;     // id awaiting the second click
  let rowMsg = null;       // { id, ok, text } — the result of a test or a failed removal
  let lastSig = '';

  async function load({ quiet = false } = {}) {
    try {
      const [next, sent] = await Promise.all([
        api.get('/notifications', null, { signal: ctx.signal }),
        api.get('/notifications/history', { limit: 25 }, { signal: ctx.signal }),
      ]);
      data = next;
      history = sent;
    } catch (e) {
      if (isAbort(e) || e.status === 401 || e.status === 403) return;
      if (!quiet) mount(root, errorState(e, load));
      return;
    }
    const sig = JSON.stringify([data, history]);
    if (editing !== null && quiet) return;   // a background refresh must not wipe what is being typed
    if (sig === lastSig && quiet) return;
    lastSig = sig;
    render();
  }

  const channelOf = (key) => data.catalogue.channels.find((c) => c.key === key) || data.catalogue.channels[0];

  function statusOf(t) {
    if (!t.enabled) return h('span', { class: 'sev sev-info' }, icon('minus', 13), 'Switched off');
    if (t.last_error) return h('span', { class: 'sev sev-critical' }, icon('alert', 13), h('span', null, 'Last message did not arrive: ', t.last_error));
    if (t.last_ok_at) return h('span', { class: 'sev sev-good' }, icon('check', 13),
      h('span', null, 'Took the last message ', h('span', { title: dateTime(t.last_ok_at) }, relTime(t.last_ok_at))));
    return h('span', { class: 'sev sev-info' }, icon('clock', 13), 'Nothing sent yet');
  }

  function row(t) {
    const act = async (fn) => {
      try { await fn(); rowMsg = null; } catch (e) { rowMsg = { id: t.id, ok: false, text: e.message }; }
      removing = null; lastSig = '';
      await load();
    };
    const test = h('button', { type: 'button', class: 'btn btn-sm' }, 'Test');
    test.addEventListener('click', async () => {
      setBusy(test, true, 'Sending…');
      try {
        const r = await api.post(`/notifications/targets/${t.id}/test`);
        rowMsg = { id: t.id, ok: r.ok, text: r.ok ? 'Test message sent.' : r.error };
      } catch (e) { rowMsg = { id: t.id, ok: false, text: e.message }; }
      setBusy(test, false);
      lastSig = '';
      await load();
    });
    const actions = removing === t.id
      ? [h('span', { class: 'muted' }, 'Remove this destination?'),
        h('button', { type: 'button', class: 'btn btn-sm btn-danger', onClick: (e) => { setBusy(e.currentTarget, true, 'Removing…'); act(() => api.del(`/notifications/targets/${t.id}`)); } }, 'Remove'),
        h('button', { type: 'button', class: 'btn btn-sm btn-ghost', onClick: () => { removing = null; render(); } }, 'Cancel')]
      : [test,
        h('button', { type: 'button', class: 'btn btn-sm', onClick: () => { editing = t.id; removing = null; rowMsg = null; render(); } }, 'Edit'),
        h('button', { type: 'button', class: 'btn btn-sm btn-ghost btn-danger-text', onClick: () => { removing = t.id; render(); } }, 'Remove…')];
    const ticked = t.events.length;
    const total = data.catalogue.events.length;
    return h('li', { class: 'conn-row' },
      h('div', { class: 'conn-main' },
        h('div', { class: 'conn-name' }, h('strong', null, t.name), h('span', { class: 'chip' }, t.label),
          t.scope === 'me' ? h('span', { class: 'chip' }, 'Yours') : t.owner_name ? h('span', { class: 'chip' }, `${t.owner_name}’s`) : null),
        h('div', { class: 'conn-url mono' }, t.shown),
        h('div', { class: 'conn-status' }, statusOf(t)),
        h('p', { class: 'help' }, `${ticked} of ${total} kinds of event`, t.with_addresses ? ' · addresses included' : '', t.min_severity !== 'info' ? ` · ${t.min_severity} and above` : ''),
        rowMsg && rowMsg.id === t.id
          ? (rowMsg.ok ? h('p', { class: 'sev sev-good' }, icon('check', 13), rowMsg.text) : inlineError(`notify-err-${t.id}`, rowMsg.text))
          : null),
      h('div', { class: 'conn-actions' }, actions));
  }

  function form(existing) {
    const channels = data.catalogue.channels;
    let channel = existing ? channelOf(existing.kind) : channels[0];
    const kindSel = h('select', { class: 'input', id: 'notify-kind', 'aria-describedby': 'notify-kind-help' }, channels.map((c) => h('option', { value: c.key }, c.label)));
    const kindHelp = h('p', { class: 'help', id: 'notify-kind-help' });
    const name = formField({ id: 'notify-name', label: 'Name (optional)', autocomplete: 'off', help: 'Shown in finstats: “Household channel”, “My phone”.' });
    const url = formField({ id: 'notify-url', label: 'Address', autocomplete: 'off', inputMode: 'url', help: ' ' });
    const topic = formField({ id: 'notify-topic', label: 'Topic', autocomplete: 'off', help: 'The ntfy topic to publish to. Anybody who knows it can read your notifications, so make it hard to guess.' });
    const secret = formField({ id: 'notify-secret', label: 'Token', type: 'password', autocomplete: 'new-password', help: ' ' });
    const severity = h('select', { class: 'input', id: 'notify-sev' }, data.catalogue.severities.map((s) => h('option', { value: s }, { info: 'Everything', warn: 'Warnings and alerts', alert: 'Alerts only' }[s] || s)));
    const addresses = h('input', { type: 'checkbox', id: 'notify-addresses', 'aria-describedby': 'notify-addresses-help' });
    const certs = h('input', { type: 'checkbox', id: 'notify-certs', 'aria-describedby': 'notify-certs-help' });
    const enabled = h('input', { type: 'checkbox', id: 'notify-enabled' });
    const scopeServer = h('input', { type: 'radio', name: 'notify-scope', id: 'notify-scope-server', checked: true });
    const scopeMine = h('input', { type: 'radio', name: 'notify-scope', id: 'notify-scope-me' });
    const formErr = h('div');
    const saveBtn = h('button', { type: 'submit', class: 'btn btn-primary' }, existing ? 'Save' : 'Add destination');

    // One tick per kind of event, grouped the way the catalogue groups them.
    const ticks = new Map();
    const chosen = existing ? existing.events : data.catalogue.events.filter((e) => e.group !== 'playback').map((e) => e.key);
    const groups = data.catalogue.groups.map((g) => {
      const events = data.catalogue.events.filter((e) => e.group === g.key);
      if (!events.length) return null;
      return h('div', { class: 'notify-group' },
        h('h4', { class: 'section-label' }, g.label),
        events.map((e) => {
          const box = h('input', { type: 'checkbox', id: `notify-ev-${e.key}`, checked: chosen.includes(e.key) });
          ticks.set(e.key, box);
          return h('label', { class: 'check notify-event' }, box, h('span', null, h('span', { class: 'notify-event-label' }, e.label), h('span', { class: 'help' }, e.what)));
        }));
    });

    function paintKind() {
      kindHelp.textContent = channel.what;
      url.input.placeholder = channel.example;
      url.el.querySelector('.help').textContent = channel.key === 'discord'
        ? 'Discord → Channel settings → Integrations → Webhooks → Copy Webhook URL. That URL is the password: finstats stores it and never shows it again.'
        : `The address finstats posts to, like ${channel.example}.`;
      topic.el.hidden = !channel.needs_topic;
      secret.el.hidden = !channel.secret_label;
      if (channel.secret_label) {
        secret.el.querySelector('.field-label').textContent = channel.secret_label;
        secret.el.querySelector('.help').textContent = (existing && existing.has_secret ? 'Leave empty to keep the stored one. ' : '') + (SECRET_HELP[channel.key] || '');
        secret.input.placeholder = existing && existing.has_secret ? 'Unchanged' : '';
      }
    }
    if (existing) {
      kindSel.value = existing.kind; kindSel.disabled = true;
      name.input.value = existing.name;
      url.input.placeholder = 'Unchanged';
      topic.input.value = existing.topic || '';
      severity.value = existing.min_severity;
      addresses.checked = !!existing.with_addresses;
      certs.checked = !!existing.accept_invalid_certs;
      enabled.checked = !!existing.enabled;
      scopeMine.checked = existing.scope === 'me';
      scopeServer.checked = existing.scope !== 'me';
    } else {
      enabled.checked = true;
      scopeServer.checked = !!data.can_add_server;
      scopeMine.checked = !data.can_add_server;
    }
    kindSel.addEventListener('change', () => { channel = channelOf(kindSel.value); paintKind(); });
    for (const f of [url, secret, topic]) f.input.addEventListener('input', () => f.setError(''));
    paintKind();

    const body = () => {
      const b = {
        kind: channel.key, name: name.input.value.trim(), events: [...ticks].filter(([, box]) => box.checked).map(([key]) => key),
        with_addresses: addresses.checked, min_severity: severity.value, accept_invalid_certs: certs.checked, enabled: enabled.checked,
      };
      if (!existing) b.scope = scopeMine.checked ? 'me' : 'server';
      if (url.input.value.trim()) b.url = url.input.value.trim();
      if (secret.input.value) b.secret = secret.input.value;
      if (channel.needs_topic) b.topic = topic.input.value.trim();
      return b;
    };
    function valid() {
      let ok = true;
      if (!existing && !url.input.value.trim()) { url.setError(`Enter the address, like ${channel.example}.`); ok = false; }
      if (channel.needs_topic && !topic.input.value.trim()) { topic.setError('Enter the topic to publish to.'); ok = false; }
      if (channel.secret_required && !secret.input.value && !(existing && existing.has_secret)) { secret.setError(`Enter the ${channel.secret_label}.`); ok = false; }
      if (!ok) root.querySelector('[aria-invalid="true"]').focus();
      return ok;
    }
    function place(err) {
      const text = err.message || 'Something went wrong.';
      if (/topic/i.test(text)) { topic.setError(text); topic.input.focus(); }
      else if (/token|key/i.test(text)) { secret.setError(text); secret.input.focus(); }
      else if (err.status === 400) { url.setError(text); url.input.focus(); }
      else mount(formErr, inlineError('notify-form-err', text));
    }

    const el = h('form', { class: 'conn-form', noValidate: true },
      h('h3', { class: 'conn-form-title' }, existing ? `Edit ${existing.name}` : 'Add a destination'),
      h('div', { class: 'form-grid' },
        h('div', { class: 'field' }, h('label', { class: 'field-label', htmlFor: 'notify-kind' }, 'Kind'), kindSel, kindHelp),
        name.el, url.el, topic.el, secret.el,
        !existing && data.can_add_server
          ? h('div', { class: 'field' }, h('span', { class: 'field-label' }, 'Who it is for'),
            h('label', { class: 'check' }, scopeServer, 'The server — everything you ticked, about anybody'),
            h('label', { class: 'check' }, scopeMine, 'Just me — only what I may already see'))
          : null,
        h('div', { class: 'field' }, h('label', { class: 'field-label', htmlFor: 'notify-sev' }, 'How much'), severity)),
      h('div', { class: 'field' },
        h('span', { class: 'field-label' }, 'What to send'),
        h('div', { class: 'notify-events' }, groups)),
      h('div', { class: 'field' },
        h('label', { class: 'check' }, addresses, 'Include IP addresses and places'),
        h('p', { class: 'help', id: 'notify-addresses-help' }, 'Off: a message says the place is “Oslo, Norway” but never the address it came from. On: the addresses go out too — worth thinking about for a destination somebody else runs, like Discord.'),
        isAdmin() ? h('label', { class: 'check' }, certs, 'Accept a self-signed certificate') : null,
        isAdmin() ? h('p', { class: 'help', id: 'notify-certs-help' }, 'Only for an https:// address whose certificate is your own.') : null,
        existing ? h('label', { class: 'check' }, enabled, 'Switched on') : null),
      formErr,
      h('div', { class: 'form-actions' }, saveBtn, h('button', { type: 'button', class: 'btn btn-ghost', onClick: () => { editing = null; render(); } }, 'Cancel')));
    el.addEventListener('submit', async (e) => {
      e.preventDefault();
      mount(formErr, '');
      if (!valid()) return;
      setBusy(saveBtn, true, 'Saving…');
      try {
        if (existing) await api.put(`/notifications/targets/${existing.id}`, body());
        else await api.post('/notifications/targets', body());
        editing = null; lastSig = '';
        await load();
      } catch (err) { setBusy(saveBtn, false); place(err); }
    });
    return el;
  }

  function sentTable() {
    const events = (history && history.events) || [];
    if (!events.length) return h('p', { class: 'help' }, 'Nothing has been sent yet. What finstats notices from now on appears here, with how each message went.');
    const delivery = (d) => h('div', { class: 'notify-delivery' },
      h('span', { class: ['sev', d.state === 'sent' ? 'sev-good' : d.state === 'failed' ? 'sev-critical' : 'sev-info'] },
        icon(d.state === 'sent' ? 'check' : d.state === 'failed' ? 'alert' : 'clock', 12),
        `${STATE_LABEL[d.state] || d.state}${d.target ? ' · ' + d.target : ''}`),
      d.error ? h('span', { class: 'cell-sub' }, d.error) : null);
    const went = (e) => {
      if (e.historic) return h('span', { class: 'muted' }, 'Not sent: older than a few hours');
      if (!e.deliveries.length) return h('span', { class: 'muted' }, 'Nothing was listening for it');
      return e.deliveries.map(delivery);
    };
    const line = (e) => h('tr', null,
      h('td', null, h('span', { class: 'when-cell' },
        h('time', { dateTime: new Date(e.at * 1000).toISOString(), title: dateTime(e.at) }, relTime(e.at)),
        h('span', { class: 'cell-sub' }, e.label || e.kind))),
      h('td', null, h('div', { class: 'notify-what' }, h('span', null, e.title), e.body ? h('span', { class: 'cell-sub' }, e.body) : null)),
      h('td', { class: 'notify-sent' }, went(e)));
    const table = h('table', { class: 'table' },
      h('thead', null, h('tr', null, h('th', null, 'When'), h('th', null, 'What'), h('th', { class: 'notify-sent' }, 'Sent to'))),
      h('tbody', null, events.map(line)));
    // Its own scroll box: a message and where it went are wide, and the card must not push the page sideways.
    return dataTable(table, { filter: false });
  }

  function publicUrlField() {
    if (!isAdmin()) return null;
    const field = formField({ id: 'notify-public-url', label: 'The address of finstats', autocomplete: 'off', inputMode: 'url',
      help: 'Used only to put a link in the messages finstats sends, since it cannot know from the inside how you reach it. Leave it empty and messages carry no link.' });
    field.input.value = data.public_url || '';
    field.input.placeholder = 'https://finstats.example';
    const save = h('button', { type: 'button', class: 'btn btn-sm' }, 'Save');
    save.addEventListener('click', async () => {
      setBusy(save, true, 'Saving…');
      try {
        await api.put('/settings', { public_url: field.input.value.trim() });
        field.setError('');
        lastSig = '';
        await load();
      } catch (e) { field.setError(e.message); } finally { setBusy(save, false); }
    });
    return h('div', { class: 'notify-public' }, field.el, h('div', { class: 'form-actions' }, save));
  }

  function render() {
    if (!data) return;
    const targets = data.targets || [];
    const existing = typeof editing === 'number' ? targets.find((t) => t.id === editing) : null;
    const intro = isAdmin()
      ? 'finstats sends nothing anywhere until you add a destination here, and then only the kinds of event you tick for it. Addresses and tokens are stored in finstats’ own database, are never shown again and are never part of a backup.'
      : 'Your own destination is sent only what you can already see in finstats, and it must point at a public address.';
    mount(root,
      h('p', { class: 'help' }, intro),
      publicUrlField(),
      targets.length ? h('ul', { class: 'conn-list' }, targets.map(row)) : h('p', { class: 'help' }, 'No destinations yet.'),
      editing === null
        ? h('div', { class: 'form-actions' }, h('button', { type: 'button', class: 'btn', onClick: () => { editing = 'new'; removing = null; rowMsg = null; render(); } }, icon('plus', 14), 'Add a destination'))
        : form(existing),
      h('div', null, h('h3', { class: 'section-label' }, 'Recently sent'), sentTable()));
    if (editing !== null) { const first = root.querySelector(existing ? '#notify-name' : '#notify-kind'); if (first) first.focus(); }
  }

  load();
  ctx.every(() => load({ quiet: true }), 30000, { visibleOnly: true });
  return root;
}

/** Who sees the card at all. The server enforces the same rule on every endpoint. */
export const mayNotify = () => isAdmin() || can('notify');
