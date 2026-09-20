// finstats SPA entry point.

import { h, icon } from './dom.js';
import { api, setUnauthorizedHandler } from './api.js';
import { state, resetCaches } from './state.js';
import { route, setLayout, start, navigate } from './router.js';
import { layout } from './shell.js';
import { startPrefetching } from './prefetch.js';
import { pageHeader, emptyState } from './components.js';

import dashboard, { prefetchDashboard } from './pages/dashboard.js';
import activity, { prefetchActivity } from './pages/activity.js';
import { usersPage, userPage, prefetchUsers, prefetchUser } from './pages/users.js';
import { timelinePage, prefetchTimeline } from './pages/timeline.js';
import { librariesPage, libraryPage, prefetchLibraries, prefetchLibrary } from './pages/libraries.js';
import itemPage, { prefetchItem } from './pages/item.js';
import personPage, { prefetchPerson } from './pages/person.js';
import playback, { prefetchPlayback } from './pages/playback.js';
import recapPage, { prefetchRecap } from './pages/recap.js';
import events, { prefetchEvents } from './pages/events.js';
import serverPage, { prefetchServer } from './pages/server.js';
import settings from './pages/settings.js';
import changelogPage, { prefetchChangelog } from './pages/changelog.js';
import { setupPage, loginPage } from './pages/auth.js';

route('/setup', setupPage, { bare: true });
route('/login', loginPage, { bare: true });
// `prefetch` hands the prefetcher the page's own loader, so what it fetches is found again by the page.
route('/', dashboard, { prefetch: prefetchDashboard });
route('/recap', recapPage, { prefetch: prefetchRecap });
route('/activity', activity, { prefetch: prefetchActivity });
route('/users', usersPage, { perm: 'see_everyone', prefetch: prefetchUsers });
route('/users/:id', userPage, { prefetch: prefetchUser });
route('/users/:id/timeline', timelinePage, { prefetch: prefetchTimeline });
route('/libraries', librariesPage, { prefetch: prefetchLibraries });
route('/libraries/:id', libraryPage, { prefetch: prefetchLibrary });
route('/items/:id', itemPage, { prefetch: prefetchItem });
route('/people/:id', personPage, { prefetch: prefetchPerson });
route('/playback', playback, { prefetch: prefetchPlayback });
route('/server', serverPage, { perm: 'see_server', prefetch: prefetchServer });
route('/events', events, { perm: 'see_server', prefetch: prefetchEvents });
route('/settings', settings, { perm: 'manage' });
route('/changelog', changelogPage, { prefetch: prefetchChangelog });
route('*', (ctx) => {
  ctx.title('Not found');
  ctx.root.append(pageHeader('Page not found'), emptyState('There’s nothing at this address.', 'It may have been a link to something that was removed.',
    h('a', { class: 'btn', href: '/' }, icon('home', 14), 'Go to the dashboard')));
});

setLayout(layout);

// Session expired (or never existed): drop the user and let the router's guard
// send us to /login?next=<where we were>.
setUnauthorizedHandler(() => {
  if (!state.user) return;
  state.user = null;
  resetCaches();
  navigate(location.pathname + location.search, { replace: true });
});

async function boot() {
  const app = document.getElementById('app');
  try {
    state.status = await api.get('/status', null, { quiet401: true });
    if (state.status.configured) {
      try { state.user = (await api.get('/auth/me', null, { quiet401: true })).user; } catch (e) { if (e.status !== 401) throw e; }
    }
  } catch (e) {
    app.replaceChildren(h('main', { class: 'bare' }, h('div', { class: 'auth-card' },
      h('h1', { class: 'auth-title' }, 'finstats isn’t responding'),
      h('p', { class: 'auth-sub' }, e.message),
      h('button', { type: 'button', class: 'btn btn-primary', onClick: () => location.reload() }, icon('refresh', 14), 'Try again'))));
    return;
  }
  startPrefetching();
  start();
}

boot();
