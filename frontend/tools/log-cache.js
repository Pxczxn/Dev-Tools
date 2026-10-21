// Budgets count UTF-16 code units (at most two bytes each), excluding object
// overhead. Keep the existing 1000-row limit for ordinary short output.
export const LOG_CACHE_LIMITS = Object.freeze({ lines: 1000, perService: 1024 * 1024, total: 4 * 1024 * 1024 });
const sizes = new WeakMap();
const unpinned = new Set();
function size(entries) {
  if (!sizes.has(entries)) {
    sizes.set(entries, entries.reduce((sum, entry) => sum + entry.text.length + entry.timestamp.length + entry.stream.length, 0));
  }
  return sizes.get(entries);
}

export function updateLogCache(current, serviceId, incoming, replace = false, pinned = unpinned) {
  const entries = [...(replace ? [] : current[serviceId] || []), ...incoming];
  let start = entries.length, retained = 0;
  while (start > 0 && entries.length - start < LOG_CACHE_LIMITS.lines) {
    const entry = entries[start - 1];
    const length = entry.text.length + entry.timestamp.length + entry.stream.length;
    if (retained + length > LOG_CACHE_LIMITS.perService) break;
    retained += length;
    start--;
  }
  const trimmed = entries.slice(start);
  sizes.set(trimmed, retained);
  // Updating insertion order makes untouched service caches expire first.
  const next = { ...current };
  delete next[serviceId];
  next[serviceId] = trimmed;
  let total = Object.values(next).reduce((sum, list) => sum + size(list), 0);
  const candidates = Object.keys(next).filter(id => id !== serviceId)
    .sort((a, b) => Number(pinned.has(a)) - Number(pinned.has(b)));
  for (const id of candidates) {
    if (total <= LOG_CACHE_LIMITS.total) break;
    total -= size(next[id]);
    delete next[id];
  }
  return next;
}
