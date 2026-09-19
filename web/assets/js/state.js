// App-wide state: server status, signed-in user, and the global filters.

import { store } from './dom.js';
import { api } from './api.js';

export const state = {
  status: null, // {configured, version, server_name?}
  user: null,   // {id, name, is_admin, has_image}
};

export const isAdmin = () => !!(state.user && state.user.is_admin);

export const RANGES = [
  { value: 7, label: '7d', long: 'Last 7 days' },
  { value: 30, label: '30d', long: 'Last 30 days' },
  { value: 90, label: '90d', long: 'Last 90 days' },
  { value: 365, label: '1y', long: 'Last year' },
  { value: 0, label: 'All', long: 'All time' },
];

const validDays = (v) => RANGES.some((r) => r.value === v);

/** Range comes from the URL first, then the last choice, then 30 days. */
export function readDays(query) {
  const q = query && query.get('days');
  if (q != null && q !== '' && validDays(Number(q))) return Number(q);
  const saved = Number(store.get('finstats.days', '30'));
  return validDays(saved) ? saved : 30;
}
export function saveDays(days) { store.set('finstats.days', String(days)); }

export const rangeLong = (days) => (RANGES.find((r) => r.value === days) || RANGES[1]).long;

// Small cache so every user combobox doesn't refetch the list.
let usersCache = null;
let usersAt = 0;
export async function userList(signal) {
  if (usersCache && Date.now() - usersAt < 60000) return usersCache;
  const data = await api.get('/users', null, { signal });
  usersCache = (data.users || []).slice().sort((a, b) => a.name.localeCompare(b.name));
  usersAt = Date.now();
  return usersCache;
}
export function resetCaches() { usersCache = null; }

// ---- "new version" hint: a dot on the Patch notes tab until the notes have been opened
const SEEN_KEY = 'finstats.seenVersion';
const versionSubs = new Set();
export function hasUnseenVersion() {
  const v = state.status && state.status.version;
  if (!v) return false;
  try { return localStorage.getItem(SEEN_KEY) !== v; } catch { return false; }
}
export function markVersionSeen(v) {
  try { if (v) localStorage.setItem(SEEN_KEY, v); } catch { /* private mode: the dot just stays */ }
  versionSubs.forEach((fn) => fn());
}
/** The server was updated while this tab stayed open. */
export function noteRunningVersion(v) {
  if (!v || !state.status || state.status.version === v) return;
  state.status.version = v;
  versionSubs.forEach((fn) => fn());
}
export function onVersionSeen(fn) { versionSubs.add(fn); return () => versionSubs.delete(fn); }
