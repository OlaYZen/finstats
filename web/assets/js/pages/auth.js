// /setup (first run) and /login. Submit buttons stay enabled; we validate on
// submit, show errors next to the field, and only disable while a request runs.

import { h, icon, mount } from '../dom.js';
import { api } from '../api.js';
import { state, resetCaches } from '../state.js';
import { navigate } from '../router.js';
import { setBusy, inlineError } from '../components.js';

function brand() {
  return h('div', { class: 'auth-brand' }, h('span', { class: 'brand-mark brand-mark-lg' }, icon('activity', 20)), h('span', { class: 'brand-name' }, 'finstats'));
}

function field({ id, label, type = 'text', autocomplete, placeholder, inputMode, help }) {
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

function normalizeUrl(raw) {
  let u = raw.trim();
  if (!u) return { error: 'Enter the address of your Jellyfin server, like http://jellyfin:8096.' };
  if (!/^https?:\/\//i.test(u)) u = 'http://' + u;
  try { const parsed = new URL(u); if (!parsed.hostname) throw new Error(); } catch { return { error: 'That doesn’t look like a web address. Try something like http://192.168.1.10:8096.' }; }
  return { url: u.replace(/\/+$/, '') };
}

// ---------------------------------------------------------------- /setup
export function setupPage(ctx) {
  ctx.title('Set up');
  let tested = null; // {url, server_name, version}

  const url = field({ id: 'jf-url', label: 'Jellyfin address', placeholder: 'http://jellyfin:8096', inputMode: 'url', autocomplete: 'url',
    help: 'The address finstats can reach Jellyfin on. In Docker this is often the container name.' });
  const testBtn = h('button', { type: 'submit', class: 'btn btn-primary' }, 'Test connection');
  const testResult = h('div', { 'aria-live': 'polite' });
  const step1 = h('form', { class: 'auth-form', noValidate: true }, url.el, h('div', { class: 'form-actions' }, testBtn), testResult);

  const user = field({ id: 'username', label: 'Jellyfin admin username', autocomplete: 'username' });
  const pass = field({ id: 'password', label: 'Password', type: 'password', autocomplete: 'current-password' });
  const finishBtn = h('button', { type: 'submit', class: 'btn btn-primary' }, 'Finish setup');
  const finishErr = h('div');
  const step2 = h('form', { class: 'auth-form', noValidate: true, hidden: true },
    user.el, pass.el,
    h('p', { class: 'help' }, icon('shield', 13), ' finstats signs in once to create its own API key. Your password is never stored.'),
    h('div', { class: 'form-actions' }, finishBtn), finishErr);

  const s1 = h('li', { class: 'wizard-step is-current' }, h('h2', { class: 'wizard-title' }, 'Connect to Jellyfin'), step1);
  const s2 = h('li', { class: 'wizard-step is-locked' }, h('h2', { class: 'wizard-title' }, 'Sign in as an administrator'),
    h('p', { class: 'help wizard-locked-note' }, 'Test the connection first.'), step2);

  url.input.addEventListener('input', () => {
    if (tested) { tested = null; mount(testResult, ''); step2.hidden = true; s2.classList.add('is-locked'); s1.classList.add('is-current'); }
  });

  step1.addEventListener('submit', async (e) => {
    e.preventDefault();
    const n = normalizeUrl(url.input.value);
    url.setError(n.error || null);
    if (n.error) { url.input.focus(); return; }
    url.input.value = n.url;
    setBusy(testBtn, true, 'Testing…');
    try {
      const info = await api.post('/setup/test', { url: n.url }, { quiet401: true });
      tested = { url: n.url, ...info };
      mount(testResult, h('p', { class: 'sev sev-good test-ok' }, icon('check', 14), h('span', null, 'Connected to ', h('strong', null, info.server_name || 'Jellyfin'), info.version ? h('span', { class: 'mono' }, ` · Jellyfin ${info.version}`) : null)));
      step2.hidden = false; s2.classList.remove('is-locked'); s1.classList.remove('is-current');
      user.input.focus();
    } catch (err) {
      tested = null;
      if (err.status === 409) { state.status = await api.get('/status'); navigate('/login', { replace: true }); return; }
      url.setError(`Couldn’t connect: ${err.message} Check the address and that Jellyfin is running.`);
      url.input.focus();
    } finally { setBusy(testBtn, false); }
  });

  step2.addEventListener('submit', async (e) => {
    e.preventDefault();
    mount(finishErr, '');
    const u = user.input.value.trim(), p = pass.input.value;
    user.setError(u ? null : 'Enter your Jellyfin username.');
    if (!u) { user.input.focus(); return; }
    if (!tested) { url.setError('Test the connection first.'); url.input.focus(); return; }
    setBusy(finishBtn, true, 'Setting up…');
    try {
      const res = await api.post('/setup', { url: tested.url, username: u, password: p }, { quiet401: true });
      state.user = res.user;
      try { state.status = await api.get('/status'); } catch { state.status = { ...state.status, configured: true, server_name: tested.server_name }; }
      navigate('/', { replace: true });
    } catch (err) {
      setBusy(finishBtn, false);
      if (err.status === 401) { pass.setError('Wrong username or password.'); pass.input.select(); pass.input.focus(); }
      else if (err.status === 403) { user.setError('That account isn’t a Jellyfin administrator. Setup needs an admin account.'); user.input.focus(); }
      else mount(finishErr, inlineError('setup-err', err.message));
    }
  });

  ctx.root.append(h('div', { class: 'auth-card auth-card-wide' }, brand(),
    h('h1', { class: 'auth-title' }, 'Set up finstats'),
    h('p', { class: 'auth-sub' }, 'Two steps, then finstats starts recording playback from your server.'),
    h('ol', { class: 'wizard' }, s1, s2)));
  requestAnimationFrame(() => url.input.focus());
}

// ---------------------------------------------------------------- /login
export function loginPage(ctx) {
  ctx.title('Sign in');
  const server = state.status && state.status.server_name;
  const user = field({ id: 'username', label: 'Username', autocomplete: 'username' });
  const pass = field({ id: 'password', label: 'Password', type: 'password', autocomplete: 'current-password' });
  const btn = h('button', { type: 'submit', class: 'btn btn-primary btn-block' }, 'Sign in');
  const formErr = h('div');
  const form = h('form', { class: 'auth-form', noValidate: true }, user.el, pass.el, formErr, btn);

  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    mount(formErr, '');
    const u = user.input.value.trim(), p = pass.input.value;
    user.setError(u ? null : 'Enter your Jellyfin username.');
    pass.setError(null);
    if (!u) { user.input.focus(); return; }
    setBusy(btn, true, 'Signing in…');
    try {
      const res = await api.post('/auth/login', { username: u, password: p }, { quiet401: true });
      state.user = res.user;
      resetCaches();
      const next = ctx.query.get('next');
      navigate(next && next.startsWith('/') && !next.startsWith('//') ? next : '/', { replace: true });
    } catch (err) {
      setBusy(btn, false);
      if (err.status === 401) { pass.setError('Wrong username or password.'); pass.input.select(); pass.input.focus(); }
      else if (err.status === 403) mount(formErr, inlineError('login-err', 'Only Jellyfin administrators can sign in here. An admin can allow other users under Settings → Access.'));
      else if (err.status === 429) mount(formErr, inlineError('login-err', 'Too many attempts. Wait a minute, then try again.'));
      else mount(formErr, inlineError('login-err', err.message));
    }
  });

  ctx.root.append(h('div', { class: 'auth-card' }, brand(),
    h('h1', { class: 'auth-title' }, 'Sign in'),
    h('p', { class: 'auth-sub' }, server ? ['Use your Jellyfin account on ', h('strong', null, server), '.'] : 'Use your Jellyfin account.'),
    form));
  requestAnimationFrame(() => user.input.focus());
}
