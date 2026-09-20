import { h, icon, num, duration, dateTime, relEl, relTime } from '../dom.js';
import { api } from '../api.js';
import { readDays, saveDays, can, rangeLong } from '../state.js';
import { replaceQuery } from '../router.js';
import { pageHeader, card, filterBar, dataView, sk, emptyState, statTile, segmented, pagination, avatar, openModal, setBusy, inlineError } from '../components.js';
import { bucketList } from '../charts.js';
import { dataTable } from '../tables.js';
import { worldMap, placeTip } from '../worldmap.js';

const PER_PAGE = 5;
// Status colours always ship with an icon and a word.
const SEVERITY = { high: { cls: 'sev-critical', icon: 'alert', label: 'High' }, medium: { cls: 'sev-warning', icon: 'alert', label: 'Medium' } };
const KIND = { impossible_travel: 'Impossible travel', new_country: 'New country' };

// Shared with the prefetcher, so a prefetched view has exactly the addresses the page asks for.
function loadSecurity({ days, userId, status, page }, signal) {
  const o = { signal };
  return Promise.all([
    api.get('/security', { days, user_id: userId }, o),
    api.get('/security/alerts', { status, user_id: userId, page, per_page: PER_PAGE }, o),
  ]).then(([overview, alerts]) => ({ overview, alerts }));
}
const scopeOf = (query) => ({ days: readDays(query), userId: query.get('user_id') || '', status: ['resolved', 'all'].includes(query.get('alerts')) ? query.get('alerts') : 'open',
  page: Math.max(1, Number(query.get('page')) || 1) });
export const prefetchSecurity = ({ query, signal }) => [() => loadSecurity(scopeOf(query), signal)];

const km = (n) => `${num(n)} km`;
function gapText(sec) {
  if (sec < 90) return `${Math.max(1, Math.round(sec))} s`;
  if (sec < 5400) return `${Math.round(sec / 60)} min`;
  return duration(sec);
}

function travelSentence(d) {
  return d.overlap
    ? `${km(d.distance_km)} apart at the same time`
    : `${km(d.distance_km)} in ${gapText(d.gap_s)}, which is ${num(d.speed_kmh)} km/h`;
}

function sightingFacts(label, side) {
  return h('div', { class: 'alert-side' },
    h('div', { class: 'alert-side-label' }, label),
    h('div', { class: 'alert-side-place' }, side.home ? icon('home', 13) : icon('globe', 13), side.place),
    h('div', { class: 'alert-side-what' }, side.what),
    h('div', { class: 'alert-side-meta mono' }, h('span', { title: dateTime(side.at) }, dateTime(side.at)), side.ip ? h('span', null, side.ip) : null));
}

export default function securityPage(ctx) {
  ctx.title('Security');
  const f = scopeOf(ctx.query);
  const view = h('div', { class: 'stack' });
  let map = null;

  const sync = () => replaceQuery({ days: f.days, user_id: f.userId, alerts: f.status === 'open' ? '' : f.status, page: f.page > 1 ? f.page : '' });
  const reload = () => { sync(); dv.load(); };

  function resolveDialog(a) {
    const travel = a.kind === 'impossible_travel';
    const note = h('input', { class: 'input', type: 'text', maxLength: 500, placeholder: 'On holiday, a VPN, a phone on mobile data…', id: 'alert-note', autocomplete: 'off' });
    const mute = h('input', { type: 'checkbox' });
    const error = inlineError('alert-error', '');
    error.hidden = true;
    const save = h('button', { type: 'submit', class: 'btn btn-primary' }, icon('check', 14), 'Resolve');
    const form = h('form', { class: 'stack-sm' },
      h('p', { class: 'help' }, `${a.user_name}: ${travel ? travelSentence(a.details) : 'first seen in ' + (a.details.country || a.details.country_code)}.`),
      h('div', { class: 'field' }, h('label', { class: 'field-label', for: 'alert-note' }, 'Note (optional)'), note),
      travel ? h('label', { class: 'check' }, mute, `Never report ${a.details.from.place} and ${a.details.to.place} for ${a.user_name} again`) : null,
      error,
      h('div', { class: 'form-actions' }, save, h('button', { type: 'button', class: 'btn btn-ghost', onClick: () => modal.close() }, 'Cancel')));
    const modal = openModal({ title: 'Resolve alert', body: form, initialFocus: note });
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      setBusy(save, true, 'Resolving…');
      try {
        await api.post(`/security/alerts/${a.id}/resolve`, { note: note.value, mute: mute.checked });
        modal.close();
        dv.load();
      } catch (err) {
        setBusy(save, false);
        error.hidden = false;
        error.lastChild.textContent = err.message;
      }
    });
  }

  function alertRow(a, manage) {
    const sev = SEVERITY[a.severity] || SEVERITY.medium, d = a.details || {};
    const travel = a.kind === 'impossible_travel' && d.from && d.to;
    const showBtn = d.to ? h('button', { type: 'button', class: 'btn btn-sm btn-ghost', onClick: () => map && map.show(travel ? [d.from, d.to] : [d.to], { line: !!travel }) }, icon('compass', 13), 'Show on map') : null;
    const act = !manage ? null : a.resolved_at
      ? h('button', { type: 'button', class: 'btn btn-sm btn-ghost', onClick: async (e) => { setBusy(e.currentTarget, true, 'Reopening…'); try { await api.post(`/security/alerts/${a.id}/reopen`); } finally { dv.load(); } } }, icon('refresh', 13), 'Reopen')
      : h('button', { type: 'button', class: 'btn btn-sm', onClick: () => resolveDialog(a) }, icon('check', 13), 'Resolve…');
    return h('li', { class: 'alert-row' + (a.resolved_at ? ' is-resolved' : '') },
      h('div', { class: 'alert-head' },
        h('span', { class: 'sev ' + sev.cls }, icon(sev.icon, 13), sev.label),
        h('strong', { class: 'alert-kind' }, KIND[a.kind] || a.kind),
        h('a', { class: 'user-cell', href: `/users/${a.user_id}` }, avatar(a.user_id, a.user_name, { size: 20, hasImage: a.has_image }), a.user_name),
        h('span', { class: 'alert-when' }, relEl(a.at))),
      h('p', { class: 'alert-text' }, travel ? `${d.from.place} → ${d.to.place}: ${travelSentence(d)}.`
        : `First time in ${d.country || d.country_code}${d.to && d.to.place ? ` (${d.to.place})` : ''}. Known before: ${num(d.known)} ${d.known === 1 ? 'country' : 'countries'}.`),
      h('div', { class: 'alert-sides' }, travel ? [sightingFacts('Before', d.from), sightingFacts('Then', d.to)] : d.to ? sightingFacts('Seen', d.to) : null),
      a.resolved_at ? h('p', { class: 'alert-resolved' }, icon('check', 13), `Resolved ${relTime(a.resolved_at)} by ${a.resolved_by || 'someone'}`, a.note ? `: ${a.note}` : '', a.muted ? ' · this pair no longer reports' : '') : null,
      h('div', { class: 'alert-actions' }, showBtn, act));
  }

  function alertsCard(alerts, manage) {
    const tabs = segmented({ label: 'Which alerts', size: 'seg-sm', value: f.status, options: [{ value: 'open', label: `Open${alerts.open ? ` (${num(alerts.open)})` : ''}` }, { value: 'resolved', label: 'Resolved' }, { value: 'all', label: 'All' }],
      onChange: (v) => { f.status = v; f.page = 1; reload(); } });
    const resolveAll = manage && f.status === 'open' && alerts.open > 1 && !f.userId
      ? h('button', { type: 'button', class: 'btn btn-sm btn-ghost', onClick: async (e) => { setBusy(e.currentTarget, true, 'Resolving…'); try { await api.post('/security/alerts/resolve-all'); } finally { dv.load(); } } }, icon('check', 13), 'Resolve all')
      : null;
    const body = alerts.rows.length
      ? [h('ul', { class: 'alert-list' }, alerts.rows.map((a) => alertRow(a, manage))),
        alerts.total > PER_PAGE ? pagination({ page: alerts.page, perPage: alerts.per_page, total: alerts.total, onPage: (p) => { f.page = p; reload(); } }) : null]
      : h('div', { class: 'chart-empty chart-empty-sm' }, f.status === 'open' ? 'Nothing needs a look. Impossible travel and first visits to a new country show up here.' : 'No alerts here.');
    return card({ title: 'Alerts', sub: 'A reason to look, never proof: places are city centres at best, and a VPN looks like a trip', actions: [resolveAll, tabs], body });
  }

  function mapPoints(o) {
    const pts = [];
    for (const p of o.places) {
      pts.push({ kind: p.home ? 'home' : 'remote', latitude: p.latitude, longitude: p.longitude, weight: p.plays + p.sign_ins, label: p.home ? `Home (${p.label})` : p.label, data: p,
        tip: () => placeTip(p.home ? `Home · ${p.label}` : p.label, [[p.plays, p.plays === 1 ? 'play' : 'plays'], p.sign_ins ? [p.sign_ins, p.sign_ins === 1 ? 'sign-in' : 'sign-ins'] : null,
          [p.users.slice(0, 3).map((u) => u.name).join(', ') + (p.users.length > 3 ? ` +${p.users.length - 3}` : ''), ''], [relTime(p.last_seen), 'last seen']]) });
    }
    for (const x of o.failed) {
      pts.push({ kind: 'failed', latitude: x.latitude, longitude: x.longitude, weight: x.attempts, label: x.label, data: { ...x, failed: true },
        tip: () => placeTip(x.label, [[x.attempts, x.attempts === 1 ? 'failed sign-in' : 'failed sign-ins'], [relTime(x.last_at), 'last attempt']]) });
    }
    for (const n of o.now_playing) {
      pts.push({ kind: 'live', latitude: n.latitude, longitude: n.longitude, weight: 1, label: `${n.user_name} in ${n.place}`, data: { ...n, live: true },
        tip: () => placeTip(`${n.user_name} · playing now`, [[n.series_name ? `${n.series_name} · ${n.item_name}` : n.item_name, ''], [n.home ? 'Home' : n.place, n.device_name || '']]) });
    }
    return pts;
  }

  function pickedPanel(slot, point) {
    const d = point.data;
    const head = h('div', { class: 'picked-head' }, h('strong', null, point.label),
      h('button', { type: 'button', class: 'icon-btn', 'aria-label': 'Close place details', onClick: () => slot.replaceChildren() }, icon('x', 14)));
    let body;
    if (d.failed) {
      body = [h('p', { class: 'help' }, `${num(d.attempts)} failed ${d.attempts === 1 ? 'sign-in' : 'sign-ins'}, last ${relTime(d.last_at)}.`), h('ul', { class: 'picked-list' }, d.events.map((e) => h('li', null, e))),
        can('see_server') ? h('a', { href: '/events?type=AuthenticationFailed' }, 'Open the server log') : null];
    } else if (d.live) {
      body = h('p', { class: 'help' }, `${d.user_name} is playing ${d.series_name ? d.series_name + ' · ' : ''}${d.item_name}${d.device_name ? ' on ' + d.device_name : ''}.`);
    } else {
      body = [h('p', { class: 'help' }, `${num(d.plays)} ${d.plays === 1 ? 'play' : 'plays'} (${duration(d.watch_s)})${d.sign_ins ? `, ${num(d.sign_ins)} sign-ins` : ''}${d.addresses ? ` from ${num(d.addresses)} ${d.addresses === 1 ? 'address' : 'addresses'}` : ''}. Last seen ${relTime(d.last_seen)}.`),
        h('ul', { class: 'picked-list' }, d.users.map((u) => h('li', null, h('a', { class: 'user-cell', href: `/users/${u.id}` }, avatar(u.id, u.name, { size: 20 }), u.name),
          h('span', { class: 'mono muted' }, `${num(u.plays)} ${u.plays === 1 ? 'play' : 'plays'}${u.sign_ins ? ` · ${num(u.sign_ins)} sign-ins` : ''}`))))];
    }
    slot.replaceChildren(h('div', { class: 'picked', role: 'status' }, head, body));
  }

  function placesTable(places) {
    return dataTable(h('table', { class: 'table' },
      h('thead', null, h('tr', null, h('th', null, 'Place'), h('th', null, 'People'), h('th', { 'data-first': 'desc' }, 'Plays'), h('th', { 'data-first': 'desc' }, 'Watch time'), h('th', { 'data-first': 'desc' }, 'Sign-ins'), h('th', { 'data-first': 'desc' }, 'Last seen'), h('th', { 'data-nosort': '' }, h('span', { class: 'sr-only' }, 'Map')))),
      h('tbody', null, places.map((p) => h('tr', null,
        h('td', null, h('span', { class: 'place-cell' }, icon(p.home ? 'home' : 'globe', 13), p.home ? `Home (${p.label})` : p.label)),
        h('td', { 'data-sort': p.users.length }, p.users.slice(0, 3).map((u) => u.name).join(', ') + (p.users.length > 3 ? ` +${p.users.length - 3}` : '')),
        h('td', { class: 'mono' }, num(p.plays)),
        h('td', { class: 'mono', 'data-sort': p.watch_s }, duration(p.watch_s)),
        h('td', { class: 'mono' }, num(p.sign_ins)),
        h('td', { class: 'mono nowrap', 'data-sort': p.last_seen, title: dateTime(p.last_seen) }, relTime(p.last_seen)),
        h('td', null, h('button', { type: 'button', class: 'btn btn-sm btn-ghost', 'aria-label': `Show ${p.label} on the map`, onClick: () => map && map.show([p]) }, icon('compass', 13))))))));
  }

  function noDatabase(o) {
    const btn = o.can_manage ? h('button', { type: 'button', class: 'btn btn-primary', onClick: async (e) => {
      const b = e.currentTarget;
      setBusy(b, true, 'Downloading…');
      try {
        await api.post('/security/database');
        const wait = setInterval(async () => {
          const t = (await api.get('/tasks', null, { signal: ctx.signal })).tasks.find((x) => x.id === 'geoip');
          if (t && t.state === 'running') { b.lastChild.textContent = t.message || 'Downloading…'; return; }
          clearInterval(wait);
          if (t && t.state === 'error') { setBusy(b, false); note.textContent = t.error || 'The download failed.'; } else dv.load();
        }, 1500);
        ctx.onCleanup(() => clearInterval(wait));
      } catch (err) { setBusy(b, false); note.textContent = err.message; }
    } }, icon('upload', 14), 'Download the database (about 60 MB)') : null;
    const note = h('p', { class: 'help', 'aria-live': 'polite' });
    return emptyState('No geolocation database yet',
      'Places come from a city database that finstats reads locally; no address is ever sent anywhere. Download DB-IP’s free one here, or put any MaxMind-format city file (.mmdb) into the geoip folder of the data directory. Settings → Security keeps it up to date.',
      h('div', { class: 'empty-action' }, btn, note));
  }

  const dv = dataView({
    container: view, signal: ctx.signal,
    skeleton: () => [sk.tiles(4), sk.cardBlock(120), sk.cardBlock(360)],
    fetch: () => loadSecurity(f, ctx.signal),
    render: ({ overview: o, alerts }) => {
      if (map) { map.destroy(); map = null; }
      if (!o.database) return noDatabase(o);
      const picked = h('div', { class: 'picked-slot' });
      const points = mapPoints(o);
      map = worldMap({ points, countries: o.countries.map((c) => c.code), signal: ctx.signal, onPick: (p) => pickedPanel(picked, p) });
      const away = o.places.filter((p) => !p.home);
      const failedTotal = o.failed.reduce((n, x) => n + x.attempts, 0);
      return [
        h('div', { class: 'tiles' },
          statTile({ label: 'Open alerts', value: num(o.open_alerts), hint: o.open_alerts ? 'Waiting for a look' : 'All clear' }),
          statTile({ label: 'Countries', value: num(o.countries.length), hint: rangeLong(f.days) }),
          statTile({ label: 'Places away from home', value: num(away.length), hint: rangeLong(f.days) }),
          can('see_server') && !f.userId ? statTile({ label: 'Failed sign-ins from outside', value: num(failedTotal), hint: rangeLong(f.days) }) : null),
        card({ title: 'Where people watch from', sub: o.home_known ? 'Plays, sign-ins and live streams by place; the home network is one dot' : 'Plays, sign-ins and live streams by place. Home is not on the map yet: finstats has not learned this network’s public address',
          body: points.length ? [map.el, picked] : h('div', { class: 'chart-empty' }, 'Nobody has watched from a public address in this range.') }),
        alertsCard(alerts, o.can_manage),
        h('div', { class: 'grid-heat' },
          card({ title: 'Places', cls: 'card-flush', body: o.places.length ? placesTable(o.places) : h('div', { class: 'chart-empty chart-empty-sm' }, 'Nothing in this range.') }),
          card({ title: 'Countries', sub: 'By plays', body: bucketList(o.countries.map((c) => ({ name: c.name, plays: c.plays, watch_s: 0 })), { watch: false, empty: 'Nothing in this range.' }) })),
        h('p', { class: 'attribution' },
          o.database.dbip ? [h('a', { href: 'https://db-ip.com', target: '_blank', rel: 'noopener noreferrer' }, 'IP Geolocation by DB-IP'), ' · '] : null,
          `${o.database.kind}, built ${dateTime(o.database.built_at).split(',')[0]}`,
          o.addresses_without_place ? ` · ${num(o.addresses_without_place)} ${o.addresses_without_place === 1 ? 'address has' : 'addresses have'} no known place` : '',
          ' · Map outlines: Natural Earth'),
      ];
    },
  });

  ctx.onCleanup(() => { if (map) map.destroy(); });
  ctx.root.append(pageHeader('Security', 'Where people watch from, and what does not add up'),
    filterBar({ days: f.days, userId: f.userId, signal: ctx.signal,
      onDays: (d) => { f.days = d; saveDays(d); reload(); }, onUser: (u) => { f.userId = u; f.page = 1; reload(); } }),
    view);
  dv.load();
}
