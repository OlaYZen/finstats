// finstats SPA entry point.

import { h, icon } from './dom.js';
import { api, setUnauthorizedHandler } from './api.js';
import { state, resetCaches } from './state.js';
import { route, setLayout, start, navigate } from './router.js';
import { layout } from './shell.js';
import { pageHeader, emptyState } from './components.js';

import dashboard from './pages/dashboard.js';
import activity from './pages/activity.js';
import { usersPage, userPage } from './pages/users.js';
import { librariesPage, libraryPage } from './pages/libraries.js';
import itemPage from './pages/item.js';
import playback from './pages/playback.js';
import events from './pages/events.js';
import settings from './pages/settings.js';
import { setupPage, loginPage } from './pages/auth.js';

route('/setup', setupPage, { bare: true });
route('/login', loginPage, { bare: true });
route('/', dashboard);
route('/activity', activity);
route('/users', usersPage, { admin: true });
route('/users/:id', userPage);
route('/libraries', librariesPage);
route('/libraries/:id', libraryPage);
route('/items/:id', itemPage);
route('/playback', playback);
route('/events', events, { admin: true });
route('/settings', settings, { admin: true });
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
  start();
}

boot();
