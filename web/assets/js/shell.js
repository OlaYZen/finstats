// App chrome: file-tree style sidebar, Obsidian-style status bar, mobile drawer,
// scroll-to-top, and the two layouts the router can ask for.

import { h, icon, num, relTime, dateTime, mount } from './dom.js';
import { api, isAbort } from './api.js';
import { state, isAdmin, resetCaches, hasUnseenVersion, onVersionSeen, noteRunningVersion } from './state.js';
import { navigate, onRouteChange } from './router.js';
import { avatar } from './components.js';
import { openPalette } from './palette.js';

let shell = null; // {el, content, navLinks, destroy, userId}
let bare = null;

const isMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

function navItems() {
  const me = state.user;
  return [
    { href: '/', label: 'Dashboard', icon: 'home', exact: true },
    { href: '/recap', label: 'Recap', icon: 'recap' },
    { href: '/activity', label: 'Activity', icon: 'activity' },
    isAdmin() ? { href: '/users', label: 'Users', icon: 'users' } : { href: `/users/${me.id}`, label: 'My stats', icon: 'user' },
    { href: '/libraries', label: 'Libraries', icon: 'library', also: ['/items'] },
    { href: '/playback', label: 'Playback', icon: 'sliders' },
    isAdmin() ? { href: '/server', label: 'Server', icon: 'server' } : null,
    isAdmin() ? { href: '/events', label: 'Server log', icon: 'log' } : null,
    isAdmin() ? { href: '/settings', label: 'Settings', icon: 'settings' } : null,
    { href: '/changelog', label: 'Patch notes', icon: 'tag', dot: hasUnseenVersion },
  ].filter(Boolean);
}

export function pageCommands() {
  return navItems().map((n) => ({ label: n.label, href: n.href, icon: n.icon }));
}

async function signOut(btn) {
  btn.disabled = true;
  try { await api.post('/auth/logout', {}, { quiet401: true }); } catch { /* signing out anyway */ }
  state.user = null;
  resetCaches();
  navigate('/login');
}

function buildShell() {
  const me = state.user;
  const items = navItems();
  const navLinks = items.map((n) => {
    const a = h('a', { class: 'nav-row', href: n.href }, icon(n.icon, 15), h('span', null, n.label));
    if (n.dot) {
      // "Something new here" — announced in text too, never by the dot alone.
      const dot = h('span', { class: 'nav-dot' }, h('span', { class: 'sr-only' }, 'New version'));
      const paint = () => { dot.hidden = !n.dot(); };
      paint();
      onVersionSeen(paint);
      a.append(dot);
    }
    a._item = n;
    return a;
  });

  const searchBtn = h('button', { type: 'button', class: 'search-btn', onClick: () => openPalette(), 'aria-keyshortcuts': isMac ? 'Meta+K' : 'Control+K' },
    icon('search', 14), h('span', null, 'Search…'), h('kbd', null, isMac ? '⌘K' : 'Ctrl K'));

  const logout = h('button', { type: 'button', class: 'icon-btn', 'aria-label': 'Sign out', title: 'Sign out' }, icon('logout', 15));
  logout.addEventListener('click', () => signOut(logout));

  const sidebar = h('aside', { class: 'sidebar', id: 'sidebar', 'aria-label': 'Main' },
    h('a', { class: 'brand', href: '/' }, brandMark(), h('span', { class: 'brand-name' }, 'finstats')),
    searchBtn,
    h('nav', { class: 'nav', 'aria-label': 'Pages' }, navLinks),
    h('div', { class: 'sidebar-foot' },
      h('a', { class: 'me', href: `/users/${me.id}` }, avatar(me.id, me.name, { size: 26, hasImage: me.has_image }),
        h('span', { class: 'me-text' }, h('span', { class: 'me-name' }, me.name), h('span', { class: 'me-role' }, me.is_admin ? 'Administrator' : 'Viewer'))),
      logout));

  const menuBtn = h('button', { type: 'button', class: 'icon-btn', 'aria-label': 'Open menu', 'aria-controls': 'sidebar', 'aria-expanded': 'false' }, icon('menu', 18));
  const topbar = h('header', { class: 'topbar' }, menuBtn, h('a', { class: 'brand', href: '/' }, brandMark(), h('span', { class: 'brand-name' }, 'finstats')),
    h('button', { type: 'button', class: 'icon-btn', 'aria-label': 'Search', onClick: () => openPalette() }, icon('search', 18)));
  const scrim = h('div', { class: 'scrim', hidden: true });
  const setDrawer = (open) => {
    el.classList.toggle('drawer-open', open);
    scrim.hidden = !open;
    menuBtn.setAttribute('aria-expanded', String(open));
  };
  menuBtn.addEventListener('click', () => setDrawer(!el.classList.contains('drawer-open')));
  scrim.addEventListener('click', () => setDrawer(false));

  const content = h('div', { class: 'content', id: 'content' });
  const main = h('main', { class: 'main', tabindex: -1 }, content);

  // ---- status bar
  const sbDot = h('span', { class: 'sb-dot' });
  const sbStream = h('a', { href: '/', class: 'sb-item sb-link' }, sbDot, h('span', null, 'connecting…'));
  const sbPlays = h('span', { class: 'sb-item' });
  const sbSync = h('span', { class: 'sb-item' });
  const sbVer = h('a', { class: 'sb-item sb-right sb-link', href: '/changelog', title: 'Patch notes' }, state.status && state.status.version ? 'v' + state.status.version : '');
  const statusbar = h('footer', { class: 'statusbar', role: 'status', 'aria-label': 'Collector status' }, sbStream, sbPlays, sbSync, sbVer);

  let sbTimer = null, sbAbort = null, dead = false;
  async function pollSummary() {
    if (dead) return;
    if (!document.hidden) {
      sbAbort = new AbortController();
      try {
        const sum = await api.get('/summary', null, { signal: sbAbort.signal });
        const ok = !!sum.collector_ok;
        sbDot.className = 'sb-dot ' + (ok ? 'ok' : 'bad');
        const n = sum.active_sessions || 0;
        sbStream.lastChild.textContent = ok ? `${num(n)} streaming` : 'collector offline';
        sbStream.title = ok ? 'Go to now playing' : 'finstats can’t reach Jellyfin right now. Plays are not being recorded.';
        sbPlays.textContent = `${num(sum.plays_total)} plays`;
        sbSync.textContent = sum.last_sync_at ? `synced ${relTime(sum.last_sync_at)}` : 'not synced yet';
        sbSync.title = sum.last_sync_at ? 'Last library sync: ' + dateTime(sum.last_sync_at) : '';
        if (sum.version) { sbVer.textContent = 'v' + sum.version; noteRunningVersion(sum.version); }
      } catch (e) {
        if (!isAbort(e) && e.status !== 401) {
          sbDot.className = 'sb-dot bad';
          sbStream.lastChild.textContent = 'finstats unreachable';
        }
      }
    }
    if (!dead) sbTimer = setTimeout(pollSummary, 10000);
  }
  pollSummary();

  // ---- scroll to top (sits above the status bar)
  const toTop = h('button', { type: 'button', class: 'to-top', 'aria-label': 'Scroll to top', hidden: true }, icon('arrowUp', 16));
  toTop.addEventListener('click', () => {
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    window.scrollTo({ top: 0, behavior: reduce ? 'auto' : 'smooth' });
    main.focus({ preventScroll: true });
  });
  let ticking = false;
  const onScroll = () => {
    if (ticking) return;
    ticking = true;
    requestAnimationFrame(() => {
      ticking = false;
      const show = window.scrollY > 400;
      if (show) { toTop.hidden = false; requestAnimationFrame(() => toTop.classList.add('is-visible')); }
      else { toTop.classList.remove('is-visible'); setTimeout(() => { if (window.scrollY <= 400) toTop.hidden = true; }, 160); }
    });
  };
  window.addEventListener('scroll', onScroll, { passive: true });

  const el = h('div', { class: 'app' }, topbar, sidebar, scrim, main, statusbar, toTop);

  return {
    el, content, userId: me.id + ':' + me.is_admin,
    setActive(path) {
      setDrawer(false);
      for (const a of navLinks) {
        const n = a._item;
        const on = n.exact ? path === n.href : path === n.href || path.startsWith(n.href + '/') || (n.also || []).some((p) => path.startsWith(p));
        a.classList.toggle('is-active', on);
        if (on) a.setAttribute('aria-current', 'page'); else a.removeAttribute('aria-current');
      }
    },
    destroy() {
      dead = true;
      clearTimeout(sbTimer);
      if (sbAbort) sbAbort.abort();
      window.removeEventListener('scroll', onScroll);
      el.remove();
    },
  };
}

function brandMark() {
  const m = icon('activity', 16);
  return h('span', { class: 'brand-mark' }, m);
}

/** Router hook: returns the element pages render into. */
export function layout(kind) {
  const appRoot = document.getElementById('app');
  if (kind === 'bare') {
    if (shell) { shell.destroy(); shell = null; }
    if (!bare) { bare = h('main', { class: 'bare' }); }
    if (!bare.isConnected) mount(appRoot, bare);
    return bare;
  }
  const key = state.user.id + ':' + state.user.is_admin;
  if (shell && shell.userId !== key) { shell.destroy(); shell = null; }
  if (!shell) {
    shell = buildShell();
    mount(appRoot, shell.el);
  }
  return shell.content;
}

onRouteChange((path) => { if (shell) shell.setActive(path); });

// ---- Esc steps back out of a detail page
// Overlays (modal, palette, dropdown) swallow their own Esc before it gets here.
let herePath = location.pathname;
let cameFrom = null;
onRouteChange(() => {
  if (location.pathname !== herePath) { cameFrom = herePath; herePath = location.pathname; }
});

/** Where Esc leads from this page; null on top-level pages. `back` = prefer the real history entry. */
function escTarget(path) {
  if (/^\/libraries\/[^/]+/.test(path)) return { up: '/libraries' };
  if (/^\/users\/[^/]+/.test(path)) return isAdmin() ? { up: '/users' } : null; // for everyone else this is "My stats", a top-level page
  if (/^\/items\/[^/]+/.test(path)) return { up: '/libraries', back: true };     // reached from anywhere, so return to wherever that was
  return null;
}

document.addEventListener('keydown', (e) => {
  if (e.key !== 'Escape' || e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || e.shiftKey || !state.user) return;
  const drawer = document.querySelector('.drawer-open');
  if (drawer) { const scrim = document.querySelector('.scrim'); if (scrim) scrim.click(); return; }
  // Esc belongs to whatever you are using: leave fields and keyboard-driven widgets (charts, rows) alone.
  const el = document.activeElement;
  if (el && el !== document.body && !el.matches('a, button, main')) { if (el.matches('input, textarea')) el.blur(); return; }
  const target = escTarget(location.pathname);
  if (!target) return;
  e.preventDefault();
  // Going back through history restores the list exactly as it was left (scroll position, filters).
  if (cameFrom !== null && (target.back || cameFrom === target.up)) history.back();
  else navigate(target.up);
});

// Global shortcut for the command palette.
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'k') {
    if (!state.user) return;
    e.preventDefault();
    openPalette();
  }
});
