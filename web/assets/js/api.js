// Thin fetch wrapper for the finstats API (see docs/api.md).

export class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

let onUnauthorized = () => {};
export function setUnauthorizedHandler(fn) { onUnauthorized = fn; }

export function qs(params) {
  const p = new URLSearchParams();
  for (const [k, v] of Object.entries(params || {})) {
    if (v == null || v === '' || v === false) continue;
    p.set(k, v);
  }
  const str = p.toString();
  return str ? '?' + str : '';
}

// ---- view cache (stale-while-revalidate)
// A view is identified by the GET requests it makes. dataView() records those while it calls a
// page's fetch(), so the same page with the same filters finds its last result again and can
// paint it at once while fresh data loads. Memory only; cleared whenever the signed-in user changes.
let recording = null;
const viewCache = new Map();
const VIEW_CACHE_MAX = 150; // a view is a few kB of JSON; the warm-up alone remembers around 35
export function recordRequests(fn) {
  const urls = [];
  recording = urls;
  try { return { result: fn(), key: urls.join('\n') }; } finally { recording = null; }
}
export const viewCacheGet = (key) => (key ? viewCache.get(key) : undefined);
export function viewCacheSet(key, data) {
  if (!key) return;
  viewCache.delete(key); // re-insert: Map order doubles as least-recently-used order
  viewCache.set(key, data);
  if (viewCache.size > VIEW_CACHE_MAX) viewCache.delete(viewCache.keys().next().value);
}
const cleared = new Set();
/** Told whenever what is remembered is thrown away (sign-out, any write). */
export function onViewCacheCleared(fn) { cleared.add(fn); }
export function clearViewCache() {
  viewCache.clear();
  for (const fn of cleared) fn();
}

// GETs made on behalf of the prefetcher (recognised by its AbortSignal) can be joined: a page that asks for
// the same address while one is on its way waits for that answer instead of asking again. Only those:
// joining a page's own request would tie the newcomer to a signal that dies with the page before it.
let sharedSignal = null;
const inflight = new Map();
export function shareRequestsOf(signal) { sharedSignal = signal; inflight.clear(); }

function request(method, path, { body, signal, params, quiet401 = false } = {}) {
  if (method === 'GET' && recording) recording.push(path + qs(params));
  else if (method !== 'GET') clearViewCache(); // something was changed; what we remember may be wrong
  if (method !== 'GET') return send(method, path, { body, signal, params, quiet401 });
  const url = path + qs(params);
  if (inflight.has(url)) return inflight.get(url);
  const sent = send(method, path, { body, signal, params, quiet401 });
  if (signal && signal === sharedSignal) {
    inflight.set(url, sent);
    const done = () => { if (inflight.get(url) === sent) inflight.delete(url); };
    sent.then(done, done);
  }
  return sent;
}

async function send(method, path, { body, signal, params, quiet401 = false } = {}) {
  const init = { method, signal, credentials: 'same-origin', headers: { Accept: 'application/json' } };
  if (body !== undefined) {
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  let res;
  try {
    res = await fetch('/api' + path + qs(params), init);
  } catch (e) {
    if (e.name === 'AbortError') throw e;
    throw new ApiError(0, 'Can’t reach finstats. Check that the server is running and try again.');
  }
  let data = null;
  const text = await res.text();
  if (text) { try { data = JSON.parse(text); } catch { data = null; } }
  if (!res.ok) {
    const err = new ApiError(res.status, (data && data.error) || `Request failed (${res.status})`);
    if (res.status === 401 && !quiet401) onUnauthorized();
    throw err;
  }
  return data;
}

export const api = {
  get: (path, params, opts) => request('GET', path, { params, ...opts }),
  post: (path, body, opts) => request('POST', path, { body: body ?? {}, ...opts }),
  put: (path, body, opts) => request('PUT', path, { body, ...opts }),
  del: (path, opts) => request('DELETE', path, opts),
};

export const isAbort = (e) => e && e.name === 'AbortError';

/**
 * For secondary data a page can live without (newer endpoints, optional cards):
 * resolves to null on failure instead of taking the whole page down. Aborts and
 * expired sessions still propagate.
 */
export function soft(promise) {
  return promise.catch((e) => { if (isAbort(e) || e.status === 401) throw e; return null; });
}

export const imgItem = (id, w = 120, kind = 'primary') => `/api/img/item/${encodeURIComponent(id)}?kind=${kind}&w=${w}`;
export const imgUser = (id, w = 96) => `/api/img/user/${encodeURIComponent(id)}?w=${w}`;

/**
 * Raw-body upload with progress (fetch can't report upload progress).
 * Returns {promise, abort}.
 */
export function uploadRaw(path, file, onProgress) {
  const xhr = new XMLHttpRequest();
  const promise = new Promise((resolve, reject) => {
    xhr.open('POST', '/api' + path);
    xhr.setRequestHeader('Content-Type', 'application/octet-stream');
    xhr.setRequestHeader('Accept', 'application/json');
    xhr.upload.onprogress = (e) => { if (e.lengthComputable) onProgress(e.loaded / e.total, e.loaded, e.total); };
    xhr.onload = () => {
      let data = null;
      try { data = JSON.parse(xhr.responseText); } catch { /* not json */ }
      if (xhr.status >= 200 && xhr.status < 300) resolve(data);
      else reject(new ApiError(xhr.status, (data && data.error) || `Upload failed (${xhr.status})`));
    };
    xhr.onerror = () => reject(new ApiError(0, 'The upload was interrupted. Check your connection and try again.'));
    xhr.onabort = () => reject(new ApiError(-1, 'Upload cancelled.'));
    xhr.send(file);
  });
  return { promise, abort: () => xhr.abort() };
}
