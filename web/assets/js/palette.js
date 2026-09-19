// Command palette (Ctrl+Space): jump to a page, a user, or anything in the library.

import { h, icon, debounce, mount } from './dom.js';
import { api, isAbort } from './api.js';
import { navigate } from './router.js';
import { openModal, poster, avatar } from './components.js';
import { pageCommands } from './shell.js';

let isOpen = false;

export function openPalette() {
  if (isOpen) return;
  isOpen = true;

  const pages = pageCommands();
  let results = [];   // flat list of {label, sub, href, kind, node}
  let active = 0;
  let abort = null;
  let lastQuery = '';

  const input = h('input', { class: 'palette-input', type: 'text', placeholder: 'Search movies, shows, users, pages…', autocomplete: 'off',
    role: 'combobox', 'aria-expanded': 'true', 'aria-controls': 'palette-list', 'aria-autocomplete': 'list', 'aria-label': 'Search' });
  const list = h('ul', { class: 'palette-list', id: 'palette-list', role: 'listbox' });
  const hint = h('div', { class: 'palette-foot' },
    h('span', null, h('kbd', null, '↑'), h('kbd', null, '↓'), ' to move'), h('span', null, h('kbd', null, '↵'), ' to open'), h('span', null, h('kbd', null, 'Esc'), ' to close'));
  const body = h('div', { class: 'palette' }, h('div', { class: 'palette-search' }, icon('search', 16), input), list, hint);

  const modal = openModal({ title: 'Search', body, bare: true, cls: 'modal-palette', initialFocus: input,
    onClose: () => { isOpen = false; if (abort) abort.abort(); search.cancel(); } });

  function go(r) { modal.close(); navigate(r.href); }

  function paint(groups, emptyText) {
    results = groups.flatMap((g) => g.rows);
    active = Math.min(active, Math.max(0, results.length - 1));
    if (!results.length) { mount(list, h('li', { class: 'palette-empty' }, emptyText || 'No results')); input.removeAttribute('aria-activedescendant'); return; }
    let i = 0;
    mount(list, groups.filter((g) => g.rows.length).map((g) => [
      h('li', { class: 'palette-group', role: 'presentation' }, g.title),
      g.rows.map((r) => {
        const idx = i++;
        return h('li', { id: 'pal-' + idx, role: 'option', class: ['palette-opt', idx === active && 'is-active'], 'aria-selected': String(idx === active),
          onPointermove: () => { if (active !== idx) setActive(idx); },
          onClick: () => go(r) }, r.thumb, h('span', { class: 'palette-label' }, r.label), r.sub ? h('span', { class: 'palette-sub' }, r.sub) : null);
      })]));
    input.setAttribute('aria-activedescendant', 'pal-' + active);
  }

  function setActive(idx) {
    const opts = list.querySelectorAll('.palette-opt');
    opts[active]?.classList.remove('is-active'); opts[active]?.setAttribute('aria-selected', 'false');
    active = idx;
    opts[active]?.classList.add('is-active'); opts[active]?.setAttribute('aria-selected', 'true');
    opts[active]?.scrollIntoView({ block: 'nearest' });
    input.setAttribute('aria-activedescendant', 'pal-' + active);
  }

  // every typed word has to be in the label, in any order
  const pageRows = (q) => pages.filter((p) => !q || q.split(/\s+/).filter(Boolean).every((w) => p.label.toLowerCase().includes(w)))
    .map((p) => ({ label: p.label, href: p.href, thumb: h('span', { class: 'palette-icon' }, icon(p.icon, 15)) }));

  const typeName = { Movie: 'Movie', Series: 'Series', MusicAlbum: 'Album', Audio: 'Track', Episode: 'Episode' };

  const search = debounce(async (q) => {
    if (abort) abort.abort();
    abort = new AbortController();
    try {
      const data = await api.get('/search', { q, limit: 12 }, { signal: abort.signal });
      if (q !== lastQuery) return;
      active = 0;
      paint([
        { title: 'Pages', rows: pageRows(q.toLowerCase()) },
        { title: 'Users', rows: (data.users || []).map((u) => ({ label: u.name, href: `/users/${u.id}`, thumb: avatar(u.id, u.name, { size: 24 }) })) },
        { title: 'Library', rows: (data.items || []).map((it) => ({ label: it.name, href: `/items/${it.id}`,
          sub: [typeName[it.type] || it.type, it.sub || it.year].filter(Boolean).join(' · '), thumb: poster(it.image_item_id || it.id, it.name, { w: 120, cls: 'poster-xs' }) })) },
      ], `Nothing matches “${q}”.`);
    } catch (e) {
      if (isAbort(e) || e.status === 401) return;
      paint([{ title: 'Pages', rows: pageRows(q.toLowerCase()) }], 'Search is unavailable right now.');
    }
  }, 150);

  input.addEventListener('input', () => {
    const q = input.value.trim();
    lastQuery = q;
    active = 0;
    if (!q) { search.cancel(); if (abort) abort.abort(); paint([{ title: 'Pages', rows: pageRows('') }]); return; }
    paint([{ title: 'Pages', rows: pageRows(q.toLowerCase()) }], 'Searching…');
    search(q);
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); if (results.length) setActive((active + 1) % results.length); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); if (results.length) setActive((active - 1 + results.length) % results.length); }
    else if (e.key === 'Enter') { e.preventDefault(); if (results[active]) go(results[active]); }
  });

  paint([{ title: 'Pages', rows: pageRows('') }]);
}
