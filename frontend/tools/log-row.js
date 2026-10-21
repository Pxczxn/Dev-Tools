// Entry identity stays stable when the bounded history drops its oldest rows.
// Text/timestamp keys can collide when a process emits identical lines at once.
const keys = new WeakMap();
let nextKey = 0;
export function logKey(entry) {
  if (!keys.has(entry)) keys.set(entry, ++nextKey);
  return keys.get(entry);
}

export function createLogRow(React, jsx) {
  return React.memo(function LogRow({ entry, precise = false }) {
    const date = new Date(entry.timestamp);
    const time = precise
      ? `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}:${String(date.getSeconds()).padStart(2, '0')}.${String(date.getMilliseconds()).padStart(3, '0')}`
      : date.toLocaleTimeString();
    const error = entry.stream === 'stderr';
    return jsx('div', {
      className: `log-line ${error ? 'log-stderr' : ''}`,
      children: [
        jsx('span', { className: 'log-time', children: time }, 'time'),
        jsx('span', { className: 'log-stream', style: { color: error ? 'var(--color-danger)' : 'var(--accent-color)' }, children: error ? 'ERR' : 'OUT' }, 'stream'),
        jsx('span', { className: 'log-text', children: entry.text }, 'text'),
      ],
    });
  });
}
