// History-API router with a per-page lifecycle: every page gets an AbortSignal
// and timer helpers that are torn down on the next navigation.

import { state, can } from './state.js';

const routes = [];
let current = null; // {cleanup: [], abort}
let layoutFn = null;
const listeners = new Set();

export function route(pattern, page, opts = {}) {
  const keys = [];
  if (pattern === '*') { routes.push({ pattern, re: /^\b$/, keys, page, ...opts }); return; } // fallback: never matched directly
  const re = new RegExp('^' + pattern.replace(/:[a-z_]+/gi, (m) => { keys.push(m.slice(1)); return '([^/]+)'; }) + '/?$');
  routes.push({ pattern, re, keys, page, ...opts });
}

export function setLayout(fn) { layoutFn = fn; }
export function onRouteChange(fn) { listeners.add(fn); }

function match(pathname) {
  for (const r of routes) {
    const m = r.re.exec(pathname);
    if (m) {
      const params = {};
      r.keys.forEach((k, i) => { params[k] = decodeURIComponent(m[i + 1]); });
      return { route: r, params };
    }
  }
  return null;
}

/** The route behind an in-app address, if the signed-in user may open it. `key` identifies the address. */
export function resolve(href) {
  let url;
  try { url = new URL(href, location.origin); } catch { return null; }
  if (url.origin !== location.origin) return null;
  const m = match(url.pathname);
  if (!m || !state.user || (m.route.perm && !can(m.route.perm))) return null;
  return { ...m, query: url.searchParams, key: url.pathname + url.search };
}

/** Where should this path actually go, given setup/auth state? */
function guard(url) {
  const path = url.pathname;
  if (state.status && !state.status.configured) return path === '/setup' ? null : '/setup';
  if (path === '/setup') return '/';
  if (!state.user) {
    if (path === '/login') return null;
    const next = path + url.search;
    return '/login' + (next && next !== '/' ? '?next=' + encodeURIComponent(next) : '');
  }
  if (path === '/login') return '/';
  return null;
}

function teardown() {
  if (!current) return;
  current.abort.abort();
  for (const fn of current.cleanup.splice(0)) { try { fn(); } catch (e) { console.error(e); } }
  current = null;
}

export function navigate(to, { replace = false, scroll = true } = {}) {
  const url = new URL(to, location.origin);
  const target = url.pathname + url.search + url.hash;
  if (replace) history.replaceState(null, '', target);
  else if (target !== location.pathname + location.search + location.hash) history.pushState(null, '', target);
  render(scroll);
}

/** Update the query string without re-mounting the page. */
export function replaceQuery(params) {
  const p = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v != null && v !== '') p.set(k, v);
  const str = p.toString();
  history.replaceState(null, '', location.pathname + (str ? '?' + str : '') + location.hash);
}

function render(scroll = true) {
  const url = new URL(location.href);
  const redirect = guard(url);
  if (redirect) { history.replaceState(null, '', redirect); return render(scroll); }

  let m = match(url.pathname);
  if (m && m.route.perm && !can(m.route.perm)) {
    history.replaceState(null, '', '/');
    return render(scroll);
  }
  if (!m) m = { route: routes.find((r) => r.pattern === '*'), params: {} };

  teardown();
  const abort = new AbortController();
  const cleanup = [];
  current = { abort, cleanup };

  const root = layoutFn(m.route.bare ? 'bare' : 'app');
  root.replaceChildren();

  const ctx = {
    params: m.params,
    query: url.searchParams,
    path: url.pathname,
    signal: abort.signal,
    root,
    onCleanup: (fn) => cleanup.push(fn),
    /** Poll while mounted. visibleOnly pauses while the tab is hidden. Returns a function to change the interval. */
    every(fn, ms, { visibleOnly = false } = {}) {
      let timer = null;
      let interval = ms;
      let running = false;
      const tick = async () => {
        if (abort.signal.aborted) return;
        running = true;
        if (!(visibleOnly && document.hidden)) { try { await fn(); } catch (e) { if (e.name !== 'AbortError') console.warn(e); } }
        running = false;
        if (!abort.signal.aborted) timer = setTimeout(tick, interval);
      };
      timer = setTimeout(tick, interval);
      cleanup.push(() => clearTimeout(timer));
      return (next) => {
        if (next === interval) return;
        interval = next;
        if (!running && !abort.signal.aborted) { clearTimeout(timer); timer = setTimeout(tick, interval); }
      };
    },
    title(t) { document.title = t ? `${t} · finstats` : 'finstats'; },
  };

  if (scroll) window.scrollTo(0, 0);
  for (const fn of listeners) fn(url.pathname);
  try {
    m.route.page(ctx);
  } catch (e) {
    console.error(e);
    root.textContent = 'This page failed to render. Reload to try again.';
  }
  if (location.hash) {
    requestAnimationFrame(() => document.getElementById(location.hash.slice(1))?.scrollIntoView());
  }
}

export function start() {
  window.addEventListener('popstate', () => render(false));
  document.addEventListener('click', (e) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const a = e.target.closest && e.target.closest('a[href]');
    if (!a || a.target || a.hasAttribute('download')) return;
    const href = a.getAttribute('href');
    if (!href || !href.startsWith('/') || href.startsWith('/api/')) return;
    e.preventDefault();
    navigate(href);
  });
  render(false);
}
