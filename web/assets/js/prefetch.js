// Makes pages open at once by fetching their data before they are asked for. Two triggers:
//   • warm-up: shortly after the app has settled, the top-level pages (and each library and the most active
//     people) are fetched one after the other while the browser is idle;
//   • intent: resting the pointer on a link, focusing it or touching it fetches what that page needs.
// Results land in the same view cache dataView() reads, under the same key, because a route's `prefetch`
// calls the very loader its page uses. Titles are only ever fetched on intent: there are thousands of them.
import { recordRequests, viewCacheSet, onViewCacheCleared, shareRequestsOf } from './api.js';
import { resolve, onRouteChange } from './router.js';
import { state, can } from './state.js';

const FRESH_MS = 60_000;  // an address fetched this recently is not fetched again
const INTENT_MS = 90;     // how long the pointer has to rest on a link
const WARM_DELAY_MS = 1200;
const MAX_EACH = 12;      // libraries / people warmed individually
const WARM_EVERY_MS = 120_000; // a write empties the cache; do not answer every one with a full warm-up

const fetched = new Map(); // address → when
let abort = new AbortController();
let generation = 0;        // bumped whenever the cache is thrown away: late answers from before are dropped
let warmedFor = -1, warmedAt = 0;
shareRequestsOf(abort.signal);

onViewCacheCleared(() => {
  generation++;
  fetched.clear();
  abort.abort();
  abort = new AbortController();
  shareRequestsOf(abort.signal);
});

const frugal = () => { const c = navigator.connection; return !!(c && (c.saveData || /(^|-)2g$/.test(c.effectiveType || ''))); };
const here = () => location.pathname + location.search;

/** Fetch what the page at `href` needs and remember it. Resolves to the loaded data (one entry per view), or null. */
export async function prefetch(href) {
  const r = resolve(href);
  if (!r || !r.route.prefetch || r.key === here()) return null;
  if (Date.now() - (fetched.get(r.key) || 0) < FRESH_MS) return null;
  fetched.set(r.key, Date.now());
  const gen = generation;
  try {
    const jobs = r.route.prefetch({ params: r.params, query: r.query, signal: abort.signal });
    return await Promise.all(jobs.map(async (job) => {
      const { result, key } = recordRequests(job);
      const data = await result;
      if (gen === generation && state.user) viewCacheSet(key, data);
      return data;
    }));
  } catch {
    if (gen === generation) fetched.delete(r.key); // a failure is not remembered: the page itself will say what is wrong
    return null;
  }
}

const idle = () => new Promise((done) => ('requestIdleCallback' in window ? requestIdleCallback(() => done(), { timeout: 2000 }) : setTimeout(done, 200)));
const visible = () => new Promise((done) => {
  if (!document.hidden) { done(); return; }
  const on = () => { if (!document.hidden) { document.removeEventListener('visibilitychange', on); done(); } };
  document.addEventListener('visibilitychange', on);
});

async function warmUp() {
  const gen = generation, me = state.user;
  if (!me || frugal()) return;
  const step = async (href) => { await visible(); await idle(); return gen === generation && state.user === me ? prefetch(href) : null; };
  const top = ['/', `/users/${me.id}`, '/activity', '/libraries', '/users', '/playback', `/users/${me.id}/timeline`, '/changelog', '/server', '/events', '/recap', '/pipeline'];
  const got = {};
  for (const href of top) got[href] = await step(href);
  // Few enough to fetch one by one. Films, shows and episodes are not: they wait for a pointer.
  const libraries = ((got['/libraries'] || [])[0] || {}).libraries || [];
  for (const l of libraries.filter((x) => !x.removed).slice(0, MAX_EACH)) await step(`/libraries/${l.id}`);
  const people = can('see_everyone') ? ((got['/users'] || [])[0] || {}).users || [] : [];
  for (const u of people.filter((x) => x.id !== me.id).sort((a, b) => (b.watch_s || 0) - (a.watch_s || 0)).slice(0, MAX_EACH)) await step(`/users/${u.id}`);
}

const linkOf = (e) => {
  const a = e.target && e.target.closest && e.target.closest('a[href]');
  if (!a || a.target || a.hasAttribute('download')) return null;
  const href = a.getAttribute('href');
  return href && href.startsWith('/') && !href.startsWith('/api/') ? href : null;
};

export function startPrefetching() {
  // The first page gets the network to itself; everything else follows once it has settled.
  onRouteChange(() => {
    if (!state.user || warmedFor === generation || Date.now() - warmedAt < WARM_EVERY_MS) return;
    warmedFor = generation;
    warmedAt = Date.now();
    setTimeout(() => { warmUp().catch(() => {}); }, WARM_DELAY_MS);
  });

  let timer = null;
  document.addEventListener('pointerover', (e) => {
    if (e.pointerType !== 'mouse' || frugal()) return;
    const href = linkOf(e);
    clearTimeout(timer);
    if (href) timer = setTimeout(() => prefetch(href), INTENT_MS);
  }, { passive: true });
  document.addEventListener('focusin', (e) => { const href = linkOf(e); if (href && !frugal()) prefetch(href); });
  document.addEventListener('touchstart', (e) => { const href = linkOf(e); if (href && !frugal()) prefetch(href); }, { passive: true });
}
