import { createCollector, parseQuery, routePath } from './model.js';

const byId = id => document.getElementById(id);
const escape = value => String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const collector = createCollector();
const badge = byId('xd-errbadge');
const errors = byId('xd-errpanel');
const search = byId('xd-search');
const input = byId('xd-searchinput');
const results = byId('xd-results');
const resultList = byId('xd-resultlist');
let requestId = 0;
let serviceNames = new Map();
let renderPending = false;

export async function ipc(command, args = {}) {
  const bridge = window.__TAURI_INTERNALS__;
  if (!bridge?.invoke) throw new Error('桌面连接尚未就绪，请在桌面应用中使用此功能');
  return bridge.invoke(command, args);
}

function toggle(panel, trigger, open) {
  panel.hidden = !open;
  trigger.setAttribute('aria-expanded', String(open));
}
function closeResults() { requestId++; toggle(results, input, false); }
function closeErrors() { toggle(errors, badge, false); }
function renderErrors() {
  const total = collector.items.reduce((n, item) => n + item.count, 0);
  byId('xd-errcount').textContent = String(total);
  badge.classList.toggle('has-errors', total > 0);
  badge.setAttribute('aria-label', `报错收集，${collector.items.length} 类，共 ${total} 次`);
  byId('xd-errsummary').textContent = total ? `${collector.items.length} 类问题 · 共出现 ${total} 次` : '暂无报错，正在监听服务日志';
  byId('xd-errclear').disabled = !total;
  // 保留用户已展开的原文；新日志只更新计数和排序。
  const expanded = new Set([...byId('xd-errlist').querySelectorAll('details[open]')].map(node => node.dataset.key));
  byId('xd-errlist').innerHTML = collector.items.length ? collector.items.map(item => {
    const key = JSON.stringify([item.serviceId, item.text]);
    const date = new Date(item.lastAt);
    const time = Number.isNaN(date.getTime()) ? item.lastAt : date.toLocaleTimeString([], { hour12: false });
    return `<article class="xd-err-item"><div class="xd-err-hint">${escape(item.hint)}</div>
      <div class="xd-err-meta"><span title="${escape(item.serviceId)}">${escape(serviceNames.get(item.serviceId) || item.serviceId)}</span><time>${escape(time)}</time><span class="xd-err-repeat">×${item.count}</span></div>
      <details data-key="${escape(key)}" ${expanded.has(key) ? 'open' : ''}><summary>查看日志原文</summary><pre>${escape(item.text)}</pre></details></article>`;
  }).join('') : '<p class="xd-note">暂未收集到错误。启动服务后，相关提示会出现在这里。</p>';
}
function scheduleErrors() {
  if (renderPending) return;
  renderPending = true;
  requestAnimationFrame(() => { renderPending = false; renderErrors(); });
}
async function refreshNames() {
  try { serviceNames = new Map((await ipc('list_services')).map(s => [s.id, s.name])); scheduleErrors(); } catch { /* 名称查询失败时保留服务 ID。 */ }
}
badge.addEventListener('click', () => {
  const open = errors.hidden;
  closeResults();
  toggle(errors, badge, open);
  if (open) { refreshNames(); byId('xd-errclose').focus(); }
});
byId('xd-errclose').addEventListener('click', () => { closeErrors(); badge.focus(); });
byId('xd-errclear').addEventListener('click', () => { collector.clear(); renderErrors(); });
byId('xd-searchclose').addEventListener('click', () => { closeResults(); input.focus(); });
document.addEventListener('pointerdown', event => {
  if (!search.contains(event.target)) closeResults();
  if (!byId('xd-errbox').contains(event.target)) closeErrors();
});
document.addEventListener('keydown', event => {
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') {
    event.preventDefault(); input.focus(); input.select(); closeErrors();
  }
  if (event.key === 'Escape') {
    if (!results.hidden) { closeResults(); input.focus(); }
    if (!errors.hidden) { closeErrors(); badge.focus(); }
  }
});

function processRow(label, detail, pid) {
  return `<div class="xd-row"><div>${escape(label)}<p class="xd-muted">${escape(detail)}</p></div>${pid > 0 ? `<button class="xd-kill" type="button" data-pid="${Number(pid)}">终止</button>` : '<span class="xd-muted">无进程信息</span>'}</div>`;
}
async function runSearch() {
  const query = parseQuery(input.value);
  const id = ++requestId;
  closeErrors();
  if (query.kind === 'empty') { closeResults(); return; }
  toggle(results, input, true);
  resultList.innerHTML = '<p class="xd-note">正在查询…</p>';
  results.setAttribute('aria-busy', 'true');
  try {
    let content;
    if (query.kind === 'invalid') content = '<p class="xd-note">请输入 1–65535 之间的有效端口。</p>';
    else if (query.kind === 'port') {
      const entries = await ipc('query_multiple_ports', { ports: [query.value] });
      const unique = [...new Map(entries.map(e => [`${e.protocol}:${e.pid}`, e])).values()];
      content = unique.length ? unique.map(e => processRow(`端口 ${e.port} · ${e.protocol.toUpperCase()}`, `PID ${e.pid}`, e.pid)).join('') : `<p class="xd-note">端口 ${query.value} 未被占用</p>`;
    } else if (query.kind === 'path') {
      const [directory, file] = await Promise.all([ipc('check_is_directory', { path: query.value }), ipc('check_is_file', { path: query.value })]);
      content = directory || file ? `<div class="xd-row"><div>${directory ? '目录' : '文件'}存在<p class="xd-muted">${escape(query.value)}</p></div><button type="button" data-open="${escape(query.value)}">打开</button></div><p class="xd-note">路径占用查询暂未实现。</p>` : `<p class="xd-note">路径不存在：${escape(query.value)}</p>`;
    } else {
      const processes = await ipc('get_process_info');
      const hits = processes.filter(p => p.name.toLowerCase().includes(query.value.toLowerCase()));
      content = hits.length ? hits.slice(0, 30).map(p => processRow(`${p.name} · PID ${p.pid}`, `${p.path || '未知路径'} · ${Math.round(p.memoryUsage / 1024 / 1024)} MB`, p.pid)).join('') + (hits.length > 30 ? '<p class="xd-note">仅显示前 30 项，请输入更具体的名称。</p>' : '') : `<p class="xd-note">没有匹配“${escape(query.value)}”的进程</p>`;
    }
    if (id === requestId) resultList.innerHTML = content;
  } catch (error) {
    if (id === requestId) resultList.innerHTML = `<p class="xd-note">查询失败：${escape(error.message || error)}</p>`;
  } finally { if (id === requestId) results.setAttribute('aria-busy', 'false'); }
}
search.addEventListener('submit', event => { event.preventDefault(); runSearch(); });
input.addEventListener('input', closeResults);
resultList.addEventListener('click', async event => {
  const button = event.target.closest('button');
  if (!button) return;
  if (button.dataset.pid && !window.confirm(`确定终止 PID ${button.dataset.pid}？未保存的工作可能丢失。`)) return;
  button.disabled = true;
  try {
    if (button.dataset.pid) { await ipc('kill_process_by_pid', { pid: Number(button.dataset.pid) }); await runSearch(); }
    else if (button.dataset.open) await ipc('open_path', { path: button.dataset.open });
  } catch (error) { button.disabled = false; button.textContent = '重试'; const note = document.createElement('p'); note.className = 'xd-note'; note.textContent = `操作失败：${error.message || error}`; button.parentElement.append(note); }
  finally { button.disabled = false; }
});

function updateRoute() {
  document.body.classList.toggle('xd-home', routePath(location) === '/');
  document.body.classList.toggle('xd-ws-page', /^\/projects\/[^/]+/.test(routePath(location)));
  byId('xd-launch-open').hidden = !/^\/projects(?:\/|$)/.test(routePath(location));
}
function routeChanged() { updateRoute(); closeResults(); closeErrors(); }
// React Router changes the hash through pushState, which emits no hashchange event.
for (const method of ['pushState', 'replaceState']) {
  const original = history[method];
  history[method] = function (...args) {
    const result = Reflect.apply(original, this, args);
    window.dispatchEvent(new Event('xd:routechange'));
    return result;
  };
}
updateRoute();
window.addEventListener('xd:routechange', routeChanged);
window.addEventListener('hashchange', routeChanged);
window.addEventListener('popstate', routeChanged);
renderErrors();
let attempts = 0;
let listenerId;
let callbackId;
async function listen() {
  const bridge = window.__TAURI_INTERNALS__;
  if (!bridge?.invoke) { if (++attempts < 200) setTimeout(listen, 50); return; }
  callbackId = bridge.transformCallback(event => { if (collector.addBatch(event?.payload)) scheduleErrors(); }, false);
  try { listenerId = await ipc('plugin:event|listen', { event: 'log:batch', target: { kind: 'Any' }, handler: callbackId }); refreshNames(); }
  catch (error) { byId('xd-errsummary').textContent = `日志监听失败：${error.message || error}`; bridge.unregisterCallback?.(callbackId); }
}
window.addEventListener('pagehide', () => {
  if (listenerId !== undefined) ipc('plugin:event|unlisten', { event: 'log:batch', eventId: listenerId }).catch(() => {});
  window.__TAURI_INTERNALS__?.unregisterCallback?.(callbackId);
});
listen();
