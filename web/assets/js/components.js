// Reusable UI pieces. All of them return DOM nodes.

import { h, icon, num, compact, duration, durationExact, durEl, relEl, initials, episodeCode, methodLabel, pct, mount, clock, shortStamp } from './dom.js';
import { api, imgItem, imgUser, isAbort, recordRequests, viewCacheGet, viewCacheSet } from './api.js';
import { RANGES, userList, can } from './state.js';
import { dataTable } from './tables.js';

// ---------------------------------------------------------------- layout bits
export function pageHeader(title, sub, right) {
  return h('header', { class: 'page-header' },
    h('div', null, h('h1', { class: 'page-title' }, title), sub ? h('p', { class: 'page-sub' }, sub) : null),
    right ? h('div', { class: 'page-header-right' }, right) : null);
}

export function card({ title, sub, actions, body, cls = '', id } = {}) {
  return h('section', { class: 'card ' + cls, id },
    title || actions ? h('div', { class: 'card-head' },
      h('div', null, title ? h('h2', { class: 'card-title' }, title) : null, sub ? h('p', { class: 'card-sub' }, sub) : null),
      actions ? h('div', { class: 'card-actions' }, actions) : null) : null,
    h('div', { class: 'card-body' }, body));
}

/** Card whose body can flip between the chart and a table of the same data. */
export function chartCard({ title, sub, controls, chart, table, cls = '' }) {
  let showTable = false;
  const body = h('div');
  const label = h('span', null, 'Table');
  const toggle = h('button', { type: 'button', class: 'btn btn-ghost btn-sm', 'aria-pressed': 'false',
    onClick: () => { showTable = !showTable; render(); } }, icon('table', 14), label);
  function render() {
    toggle.setAttribute('aria-pressed', String(showTable));
    toggle.replaceChildren(icon(showTable ? 'chart' : 'table', 14), h('span', null, showTable ? 'Chart' : 'Table'));
    mount(body, showTable ? table() : chart());
  }
  render();
  const el = card({ title, sub, actions: [controls, table ? toggle : null], body, cls: 'chart-card ' + cls });
  el.rerender = render;
  return el;
}

export function emptyState(title, text, action) {
  return h('div', { class: 'empty' }, h('p', { class: 'empty-title' }, title), text ? h('p', { class: 'empty-text' }, text) : null, action || null);
}

export function errorState(err, retry) {
  return h('div', { class: 'error-state', role: 'alert' }, icon('alert', 18),
    h('div', null, h('p', { class: 'error-title' }, 'Couldn’t load this'), h('p', { class: 'error-text' }, err.message || String(err))),
    retry ? h('button', { type: 'button', class: 'btn btn-sm', onClick: retry }, icon('refresh', 14), 'Try again') : null);
}

export function inlineError(id, text) {
  return h('p', { class: 'field-error', id, role: 'alert' }, icon('alert', 14), h('span', null, text));
}

/** A labelled input with help text and a place for its error. `setError('')` clears it. */
export function formField({ id, label, type = 'text', autocomplete, placeholder, inputMode, help }) {
  const input = h('input', { class: 'input', id, name: id, type, autocomplete, placeholder, inputMode, autocapitalize: 'none', autocorrect: 'off', spellcheck: false,
    'aria-describedby': help ? id + '-help' : null });
  const err = h('div');
  const el = h('div', { class: 'field' }, h('label', { htmlFor: id, class: 'field-label' }, label), input, help ? h('p', { class: 'help', id: id + '-help' }, help) : null, err);
  return {
    el, input,
    setError(msg) {
      input.setAttribute('aria-invalid', msg ? 'true' : 'false');
      input.setAttribute('aria-describedby', [msg ? id + '-err' : null, help ? id + '-help' : null].filter(Boolean).join(' '));
      mount(err, msg ? inlineError(id + '-err', msg) : '');
    },
  };
}

export function spinner(size = 14) { return h('span', { class: 'spinner', style: { width: size + 'px', height: size + 'px' }, 'aria-hidden': 'true' }); }

/** Put a button into / out of its busy state (disabled only while the request runs). */
export function setBusy(btn, busy, busyLabel) {
  if (busy) {
    btn._label = btn._label || Array.from(btn.childNodes);
    btn.disabled = true;
    btn.setAttribute('aria-busy', 'true');
    btn.replaceChildren(spinner(), h('span', null, busyLabel || 'Working…'));
  } else if (btn._label) {
    btn.disabled = false;
    btn.removeAttribute('aria-busy');
    btn.replaceChildren(...btn._label);
    btn._label = null;
  }
}

// ---------------------------------------------------------------- skeletons
export const sk = {
  line: (w = '60%', hgt = 12) => h('span', { class: 'sk', style: { width: w, height: hgt + 'px' } }),
  block: (hgt = 120) => h('div', { class: 'sk sk-block', style: { height: hgt + 'px' } }),
  tiles: (n = 4) => h('div', { class: 'tiles' }, Array.from({ length: n }, () =>
    h('div', { class: 'tile' }, sk.line('45%', 11), sk.line('60%', 26), sk.line('35%', 10)))),
  rows: (n = 5) => h('div', { class: 'sk-rows' }, Array.from({ length: Math.min(n, 5) }, () =>
    h('div', { class: 'sk-row' }, h('span', { class: 'sk sk-thumb' }), h('div', { class: 'sk-row-lines' }, sk.line('55%'), sk.line('30%', 10))))),
  tableRows: (n = 5) => h('div', { class: 'sk-rows' }, Array.from({ length: Math.min(n, 5) }, () =>
    h('div', { class: 'sk-row' }, sk.line('18%'), sk.line('34%'), sk.line('12%'), sk.line('14%')))),
  cardBlock: (hgt = 240, title = true) => h('section', { class: 'card' }, title ? h('div', { class: 'card-head' }, sk.line('28%', 14)) : null, h('div', { class: 'card-body' }, sk.block(hgt))),
  cardRows: (n = 5) => h('section', { class: 'card' }, h('div', { class: 'card-head' }, sk.line('28%', 14)), h('div', { class: 'card-body' }, sk.rows(n))),
};

/**
 * First load → skeleton (kept ≥300ms so it never flashes).
 * Later loads → keep the previous render, dimmed, until the new data lands.
 */
export function dataView({ container, skeleton, fetch, render, signal }) {
  let loaded = false;
  let seq = 0;
  let shownJson = null; // what is on screen, to skip re-rendering identical data
  async function load() {
    const my = ++seq;
    // Calling fetch() fires the page's GET requests synchronously; their URLs identify this view.
    const { result, key } = recordRequests(fetch);
    let skeletonAt = 0;
    let skeletonTimer = null;
    if (!loaded) {
      const remembered = viewCacheGet(key);
      if (remembered !== undefined) {
        // Been here before: show what we had at once, refresh quietly behind it.
        loaded = true;
        shownJson = JSON.stringify(remembered);
        mount(container, render(remembered));
      } else {
        // A skeleton that flashes for a few milliseconds is worse than none: only show it when
        // loading is actually slow, and once shown keep it long enough to read as deliberate.
        skeletonTimer = setTimeout(() => { if (my === seq && !loaded) { skeletonAt = performance.now(); mount(container, skeleton()); } }, 150);
      }
    } else {
      // A filter changed. If this exact view was seen before, switch to it at once.
      const remembered = viewCacheGet(key);
      const json = remembered === undefined ? null : JSON.stringify(remembered);
      if (json !== null && json !== shownJson) {
        shownJson = json;
        mount(container, render(remembered));
      } else if (json === null) {
        container.classList.add('is-stale');
      }
    }
    container.setAttribute('aria-busy', 'true');
    try {
      const data = await result;
      clearTimeout(skeletonTimer);
      if (skeletonAt) {
        const wait = 300 - (performance.now() - skeletonAt);
        if (wait > 0) await new Promise((r) => setTimeout(r, wait));
      }
      if (my !== seq || (signal && signal.aborted)) return;
      viewCacheSet(key, data);
      const json = JSON.stringify(data);
      container.classList.remove('is-stale');
      if (loaded && json === shownJson) return; // nothing changed; leave the page (and its scroll, hover, focus) alone
      if (!loaded) container.classList.add('fade-in');
      loaded = true;
      shownJson = json;
      mount(container, render(data));
    } catch (e) {
      clearTimeout(skeletonTimer);
      if (isAbort(e) || my !== seq) return;
      if (e.status === 401) return;
      container.classList.remove('is-stale');
      loaded = false;
      shownJson = null;
      mount(container, errorState(e, load));
    } finally {
      if (my === seq) container.removeAttribute('aria-busy');
    }
  }
  return { load, get loaded() { return loaded; } };
}

// ---------------------------------------------------------------- controls
/** Visible options instead of a dropdown (2–5 choices). */
export function segmented({ options, value, onChange, label, size = '' }) {
  let currentValue = value;
  const group = h('div', { class: 'seg ' + size, role: 'radiogroup', 'aria-label': label });
  const btns = options.map((o) => h('button', { type: 'button', class: 'seg-btn', role: 'radio', title: o.title || null,
    onClick: () => select(o.value, true) }, o.label));
  function paint() {
    btns.forEach((b, i) => {
      const on = options[i].value === currentValue;
      b.setAttribute('aria-checked', String(on));
      b.tabIndex = on ? 0 : -1;
    });
  }
  function select(v, fire) {
    if (v === currentValue) return;
    currentValue = v; paint();
    if (fire) onChange(v);
  }
  group.addEventListener('keydown', (e) => {
    const dir = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
    if (!dir) return;
    e.preventDefault();
    const i = options.findIndex((o) => o.value === currentValue);
    const n = (i + dir + options.length) % options.length;
    select(options[n].value, true);
    btns[n].focus();
  });
  group.append(...btns);
  paint();
  group.setValue = (v) => select(v, false);
  return group;
}

export function rangeControl(days, onChange) {
  return segmented({ label: 'Time range', value: days, onChange,
    options: RANGES.map((r) => ({ value: r.value, label: r.label, title: r.long })) });
}

/** Searchable select. options: [{value, label}] or an async loader. */
export function combobox({ value = '', onChange, placeholder = 'All users', allLabel = 'All users', load, label = 'User' }) {
  let options = [];
  let open = false, activeIdx = 0, filtered = [], selected = value;
  const uid = 'cb' + Math.random().toString(36).slice(2, 8);
  const btnLabel = h('span', { class: 'combo-label' }, placeholder);
  const btn = h('button', { type: 'button', class: 'combo-btn', 'aria-haspopup': 'listbox', 'aria-expanded': 'false', 'aria-label': label },
    icon('user', 14), btnLabel, icon('chevronDown', 14, 'combo-caret'));
  const input = h('input', { class: 'combo-input', type: 'text', placeholder: 'Type to filter…', autocomplete: 'off', spellcheck: false,
    role: 'combobox', 'aria-controls': uid, 'aria-expanded': 'true', 'aria-autocomplete': 'list', 'aria-label': 'Filter ' + label.toLowerCase() + 's' });
  const list = h('ul', { class: 'combo-list', role: 'listbox', id: uid });
  const pop = h('div', { class: 'combo-pop', hidden: true }, h('div', { class: 'combo-search' }, icon('search', 14), input), list);
  const root = h('div', { class: 'combo' }, btn, pop);

  function paintLabel() {
    const o = options.find((x) => x.value === selected);
    btnLabel.textContent = selected && o ? o.label : selected ? 'Selected user' : allLabel;
    root.classList.toggle('has-value', !!selected);
  }
  function renderList() {
    const q = input.value.trim().toLowerCase();
    const all = [{ value: '', label: allLabel }, ...options];
    filtered = q ? options.filter((o) => o.label.toLowerCase().includes(q)) : all;
    activeIdx = Math.min(activeIdx, Math.max(0, filtered.length - 1));
    if (!filtered.length) { mount(list, h('li', { class: 'combo-empty' }, 'No results')); input.removeAttribute('aria-activedescendant'); return; }
    mount(list, filtered.map((o, i) => h('li', { id: `${uid}-${i}`, role: 'option', class: ['combo-opt', i === activeIdx && 'is-active'],
      'aria-selected': String(o.value === selected),
      onPointerdown: (e) => { e.preventDefault(); choose(o); },
      onPointermove: () => { if (activeIdx !== i) { activeIdx = i; renderList(); } } },
      h('span', null, o.label), o.value === selected ? icon('check', 14) : null)));
    input.setAttribute('aria-activedescendant', `${uid}-${activeIdx}`);
    list.querySelector('.is-active')?.scrollIntoView({ block: 'nearest' });
  }
  function choose(o) {
    const changed = o.value !== selected;
    selected = o.value; paintLabel(); close(true);
    if (changed) onChange(selected);
  }
  function openPop() {
    if (open) return;
    open = true; pop.hidden = false; btn.setAttribute('aria-expanded', 'true');
    input.value = ''; activeIdx = 0; renderList(); input.focus();
    document.addEventListener('pointerdown', outside, true);
  }
  function close(refocus) {
    if (!open) return;
    open = false; pop.hidden = true; btn.setAttribute('aria-expanded', 'false');
    input.value = ''; // search is cleared on close; the button keeps the full selected label
    document.removeEventListener('pointerdown', outside, true);
    if (refocus) btn.focus();
  }
  const outside = (e) => { if (!root.contains(e.target)) close(false); };
  btn.addEventListener('click', () => (open ? close(true) : openPop()));
  input.addEventListener('input', () => { activeIdx = 0; renderList(); });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); activeIdx = Math.min(filtered.length - 1, activeIdx + 1); renderList(); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); activeIdx = Math.max(0, activeIdx - 1); renderList(); }
    else if (e.key === 'Enter') { e.preventDefault(); if (filtered[activeIdx]) choose(filtered[activeIdx]); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(true); }
    else if (e.key === 'Tab') close(false);
  });

  paintLabel();
  Promise.resolve(load()).then((opts) => { options = opts || []; paintLabel(); if (open) renderList(); }).catch(() => {});
  return root;
}

export function userCombobox({ value, onChange, signal }) {
  return combobox({ value, onChange, load: () => userList(signal).then((us) => us.map((u) => ({ value: u.id, label: u.name }))) });
}

/** One row of filters above everything they scope. */
export function filterBar({ days, onDays, userId, onUser, signal, extra = [] }) {
  return h('div', { class: 'filters', role: 'group', 'aria-label': 'Filters' },
    rangeControl(days, onDays),
    onUser && can('see_everyone') ? userCombobox({ value: userId || '', onChange: onUser, signal }) : null,
    extra);
}

export function copyButton(text, label = 'Copy') {
  let timer = null;
  const btn = h('button', { type: 'button', class: 'copy-btn', 'aria-label': label, title: label }, icon('copy', 13));
  const note = h('span', { class: 'copy-note', 'aria-live': 'polite' });
  btn.addEventListener('click', async (e) => {
    e.stopPropagation();
    let ok = true;
    try { await navigator.clipboard.writeText(text); } catch { ok = false; }
    btn.replaceChildren(icon(ok ? 'check' : 'x', 13));
    btn.classList.toggle('is-ok', ok);
    note.textContent = ok ? 'Copied' : 'Copy failed — select the text instead';
    clearTimeout(timer);
    timer = setTimeout(() => { btn.replaceChildren(icon('copy', 13)); btn.classList.remove('is-ok'); note.textContent = ''; }, 2000);
  });
  return h('span', { class: 'copy' }, btn, note);
}

/** Immediate-effect setting → toggle switch. */
export function toggle({ checked, onChange, labelledby, describedby }) {
  const btn = h('button', { type: 'button', class: 'switch', role: 'switch', 'aria-checked': String(!!checked),
    'aria-labelledby': labelledby, 'aria-describedby': describedby }, h('span', { class: 'switch-knob' }));
  btn.addEventListener('click', () => {
    const next = btn.getAttribute('aria-checked') !== 'true';
    btn.setAttribute('aria-checked', String(next));
    onChange(next, (v) => btn.setAttribute('aria-checked', String(v)));
  });
  return btn;
}

export function pagination({ page, perPage, total, onPage }) {
  const pages = Math.max(1, Math.ceil(total / perPage));
  const from = total ? (page - 1) * perPage + 1 : 0, to = Math.min(total, page * perPage);
  // Buttons stay enabled; out-of-range clicks are simply ignored (aria-disabled communicates the edge).
  const mk = (lbl, ic, target, off) => h('button', { type: 'button', class: 'btn btn-sm btn-ghost', 'aria-label': lbl, 'aria-disabled': off ? 'true' : null,
    onClick: () => { if (!off) onPage(target); } }, icon(ic, 14));
  return h('nav', { class: 'pager', 'aria-label': 'Pagination' },
    h('span', { class: 'pager-info mono' }, `${num(from)}–${num(to)} of ${num(total)}`),
    h('div', { class: 'pager-btns' }, mk('Previous page', 'chevronLeft', page - 1, page <= 1),
      h('span', { class: 'pager-page mono' }, `${page} / ${pages}`),
      mk('Next page', 'chevronRight', page + 1, page >= pages)));
}

// ---------------------------------------------------------------- media
export function poster(id, name, { w = 120, cls = '', kind = 'primary' } = {}) {
  const box = h('span', { class: 'poster ' + cls, 'aria-hidden': 'true' });
  const fallback = () => mount(box, h('span', { class: 'poster-fallback' }, initials(name)));
  if (!id) { fallback(); return box; }
  box.append(h('img', { src: imgItem(id, w, kind), alt: '', loading: 'lazy', decoding: 'async', onError: fallback }));
  return box;
}

export function avatar(id, name, { size = 28, hasImage = true } = {}) {
  const box = h('span', { class: 'avatar', style: { width: size + 'px', height: size + 'px', fontSize: Math.max(10, size * 0.38) + 'px' }, 'aria-hidden': 'true' });
  const fallback = () => mount(box, initials(name));
  if (!id || hasImage === false) { fallback(); return box; }
  box.append(h('img', { src: imgUser(id, size * 2 > 96 ? 192 : 96), alt: '', loading: 'lazy', decoding: 'async', onError: fallback }));
  return box;
}

export function methodBadge(method) {
  const cls = { DirectPlay: 'm-direct', DirectStream: 'm-stream', Transcode: 'm-transcode' }[method] || '';
  return h('span', { class: 'badge method ' + cls }, h('span', { class: 'badge-dot' }), methodLabel(method));
}

export function chip(text, title) { return h('span', { class: 'chip', title }, text); }

// ---------------------------------------------------------------- stats
function deltaEl(cur, prev) {
  if (prev == null) return null;
  if (!prev && !cur) return h('span', { class: 'delta flat' }, icon('minus', 13), 'No change');
  if (!prev) return h('span', { class: 'delta up' }, icon('trendUp', 13), 'New');
  const d = (cur - prev) / prev;
  if (Math.abs(d) < 0.005) return h('span', { class: 'delta flat' }, icon('minus', 13), 'No change');
  const up = d > 0;
  return h('span', { class: 'delta ' + (up ? 'up' : 'down') }, icon(up ? 'trendUp' : 'trendDown', 13),
    `${up ? '+' : '−'}${Math.abs(d) >= 10 ? Math.round(Math.abs(d) * 100) : (Math.abs(d) * 100).toFixed(Math.abs(d) < 0.1 ? 1 : 0)}%`);
}

export function statTile({ label, value, title, current, previous, vsLabel, spark, hint }) {
  return h('div', { class: 'tile' },
    h('div', { class: 'tile-label' }, label),
    h('div', { class: 'tile-row' }, h('div', { class: 'tile-value', title }, value), spark || null),
    h('div', { class: 'tile-foot' }, previous != null ? [deltaEl(current, previous), h('span', { class: 'tile-vs' }, vsLabel)] : hint ? h('span', { class: 'tile-vs' }, hint) : h('span', { class: 'tile-vs' }, ' ')));
}

/** Ranked poster rows for movies/series/music, avatar rows for users. */
export function topList(rows, { kind = 'items', empty = 'No plays in this range.' } = {}) {
  if (!rows || !rows.length) return h('div', { class: 'chart-empty chart-empty-sm' }, empty);
  return h('ol', { class: 'toplist' }, rows.map((r, i) => {
    const href = !r.id ? null : kind === 'users' ? `/users/${r.id}` : kind === 'libraries' ? `/libraries/${r.id}` : kind === 'items' ? `/items/${r.id}` : null;
    const thumb = kind === 'users' ? avatar(r.id, r.name, { size: 36 }) : kind === 'items' ? poster(r.image_item_id, r.name, { w: 120, cls: 'poster-sm' }) : null;
    const name = href ? h('a', { href, class: 'toplist-name' }, r.name) : h('span', { class: 'toplist-name' }, r.name);
    return h('li', { class: 'toplist-row' },
      h('span', { class: 'toplist-rank mono' }, String(i + 1)),
      thumb,
      h('div', { class: 'toplist-main' }, name,
        h('div', { class: 'toplist-sub' }, [r.sub, r.users != null ? `${num(r.users)} ${r.users === 1 ? 'user' : 'users'}` : null].filter(Boolean).join(' · ') || ' ')),
      h('div', { class: 'toplist-nums' }, durEl(r.watch_s, 'mono toplist-watch'), h('span', { class: 'mono toplist-plays' }, `${num(r.plays)} ${r.plays === 1 ? 'play' : 'plays'}`)));
  }));
}

// ---------------------------------------------------------------- plays table
export function playTitle(p, { link = true } = {}) {
  const code = episodeCode(p.season_number, p.episode_number);
  const canLink = link && p.item_exists !== false;
  const main = canLink && p.item_id ? h('a', { href: `/items/${p.item_id}`, onClick: (e) => e.stopPropagation() }, p.item_name || 'Unknown item') : h('span', null, p.item_name || 'Unknown item');
  if (!p.series_name) return h('div', { class: 'play-title' }, h('div', { class: 'play-title-main' }, main));
  const series = canLink && p.series_id ? h('a', { href: `/items/${p.series_id}`, onClick: (e) => e.stopPropagation() }, p.series_name) : h('span', null, p.series_name);
  return h('div', { class: 'play-title' }, h('div', { class: 'play-title-main' }, series),
    h('div', { class: 'play-title-sub' }, code ? h('span', { class: 'mono' }, code) : null, code ? ' · ' : null, main));
}

export function completionEl(p) {
  if (p.completion == null) return h('span', { class: 'muted mono' }, '–');
  const c = Math.max(0, Math.min(1, p.completion));
  // Where playback stopped, as a position in the title. Imported plays never recorded one.
  const stoppedAt = p.position_s != null && p.runtime_s ? `${clock(p.position_s)} / ${clock(p.runtime_s)}` : null;
  return h('span', { class: 'completion-cell' },
    h('span', { class: 'completion', title: stoppedAt ? `Stopped at ${stoppedAt}` : `Stopped at ${pct(c)}` },
      h('span', { class: 'meter', role: 'img', 'aria-label': `${pct(c)} watched` }, h('span', { class: 'meter-fill', style: { width: c * 100 + '%' } })),
      h('span', { class: 'mono' }, pct(c))),
    stoppedAt ? h('span', { class: 'cell-sub mono' }, stoppedAt) : null);
}

/**
 * rows: Play[]; onOpen(play, rowEl) opens the detail modal. `sort: {key, dir, onSort}` when the list is
 * paginated and the server does the sorting; without it the rows on screen are sorted in the browser.
 */
export function playsTable(rows, { showUser = true, onOpen, empty = 'No plays match these filters.', sort = null } = {}) {
  if (!rows || !rows.length) return emptyState(empty);
  const admin = can('see_network'); // the IP column
  // data-first: the direction a first click gives, so it matches what the browser-side tables do.
  const th = (key, label, first, cls) => h('th', { class: cls || null, 'data-key': key, 'data-first': first }, label);
  return dataTable(h('table', { class: 'table table-hover plays' },
    h('thead', null, h('tr', null,
      showUser ? th('user', 'User', 'asc') : null, th('title', 'Title', 'asc'), th('when', 'When', 'desc'), th('watched', 'Watched', 'desc', 'r'),
      th('progress', 'Progress', 'desc'), th('client', 'Client', 'asc'), th('method', 'Method', 'asc'), admin ? th('ip', 'IP address', 'asc') : null)),
    h('tbody', null, rows.map((p) => {
      const tr = h('tr', { tabindex: 0, class: p.active ? 'is-live' : '', 'aria-label': `Open details for ${p.item_name || 'play'}` },
        showUser ? h('td', null, h('a', { class: 'user-cell', href: `/users/${p.user_id}`, onClick: (e) => e.stopPropagation() }, avatar(p.user_id, p.user_name, { size: 22 }), h('span', null, p.user_name))) : null,
        h('td', { class: 'td-title' }, h('div', { class: 'title-cell' }, poster(p.image_item_id, p.series_name || p.item_name, { w: 120, cls: 'poster-xs' }), playTitle(p),
          p.group_size > 1 ? h('span', { class: 'group-mark', role: 'img', title: `Watched together · ${p.group_size} people`, 'aria-label': `Watched together by ${p.group_size} people` }, icon('users', 13)) : null)),
        h('td', { 'data-sort': p.active ? String(Date.now()) : null }, p.active ? h('span', { class: 'badge live' }, h('span', { class: 'badge-dot' }), 'Playing now')
          : h('span', { class: 'when-cell' }, relEl(p.ended_at || p.started_at), h('span', { class: 'cell-sub mono' }, shortStamp(p.ended_at || p.started_at)))),
        h('td', { class: 'r' }, durEl(p.duration_s)),
        h('td', null, completionEl(p)),
        h('td', null, h('div', { class: 'client-cell' }, h('span', null, p.client || '–'), h('span', { class: 'muted' }, p.device_name || ''))),
        h('td', null, methodBadge(p.play_method)),
        admin ? h('td', { class: 'mono' }, p.remote_ip ? h('span', { class: 'ip-cell' }, p.remote_ip,
          p.is_local == null ? null : h('span', { class: 'ip-net', role: 'img', title: p.is_local ? 'Local network' : 'Remote', 'aria-label': p.is_local ? 'Local network' : 'Remote' }, icon(p.is_local ? 'lan' : 'globe', 12))) : '–') : null);
      if (onOpen) {
        tr.addEventListener('click', (e) => { if (!e.target.closest('a,button')) onOpen(p, tr); });
        tr.addEventListener('keydown', (e) => { if ((e.key === 'Enter' || e.key === ' ') && e.target === tr) { e.preventDefault(); onOpen(p, tr); } });
      }
      return tr;
    }))), { server: sort, filter: false });
}

// ---------------------------------------------------------------- modal
let modalDepth = 0;
/** X button, Esc and click-outside all close it; focus returns to the trigger. */
export function openModal({ title, body, wide = false, onClose, initialFocus, labelId = 'modal-title', bare = false, cls = '' }) {
  const trigger = document.activeElement;
  const closeBtn = h('button', { type: 'button', class: 'icon-btn modal-x', 'aria-label': 'Close' }, icon('x', 16));
  const bodyEl = h('div', { class: bare ? '' : 'modal-body' }, body);
  const dialog = h('div', { class: ['modal', wide && 'modal-wide', cls], role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': bare ? null : labelId, 'aria-label': bare ? title : null },
    bare ? null : h('div', { class: 'modal-head' }, h('h2', { class: 'modal-title', id: labelId }, title), closeBtn), bodyEl);
  const overlay = h('div', { class: 'overlay' }, dialog);
  let closed = false;

  function close() {
    if (closed) return;
    closed = true; modalDepth--;
    document.removeEventListener('keydown', onKey, true);
    overlay.classList.add('is-closing');
    setTimeout(() => overlay.remove(), 140);
    if (!modalDepth) document.documentElement.classList.remove('no-scroll');
    if (trigger && trigger.isConnected && trigger.focus) trigger.focus();
    if (onClose) onClose();
  }
  function onKey(e) {
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(); return; }
    if (e.key !== 'Tab') return;
    const f = dialog.querySelectorAll('a[href],button:not([disabled]),input:not([disabled]),select,textarea,[tabindex]:not([tabindex="-1"])');
    if (!f.length) return;
    const first = f[0], last = f[f.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  }
  closeBtn.addEventListener('click', close);
  overlay.addEventListener('pointerdown', (e) => { if (e.target === overlay) close(); });
  document.addEventListener('keydown', onKey, true);
  document.body.append(overlay);
  document.documentElement.classList.add('no-scroll');
  modalDepth++;
  requestAnimationFrame(() => (initialFocus && initialFocus.isConnected ? initialFocus : closeBtn.isConnected ? closeBtn : dialog).focus());
  return { close, body: bodyEl, dialog };
}

// ---------------------------------------------------------------- definition grid
export function facts(pairs) {
  return h('dl', { class: 'facts' }, pairs.filter(Boolean).map(([k, v, opts]) =>
    h('div', { class: 'fact' }, h('dt', null, k), h('dd', { class: opts && opts.mono ? 'mono' : '' }, v == null || v === '' ? '–' : v))));
}

export { num, compact, duration, durationExact, api };
