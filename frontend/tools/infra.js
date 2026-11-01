import { ipc } from './panel.js';
import { routePath } from './model.js';
import { pickerOptions, selectedPath } from './infra-picker.js';
import { homeBlock, serviceListState, settingsPanel, settingsTabs, softwareTable } from './infra-board.js';

const byId = id => document.getElementById(id);
const escape = value => String(value ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const page = byId('xd-infra-page');
const local = byId('xd-infra-local');
const list = byId('xd-infra-list');
const status = byId('xd-infra-status');
let loading = false;
let programs = [];
let settingsKind = '';
let settingsTab = '';
let pageOpen = false;

function serviceRows(services) {
  if (!services.length) return '<p class="xd-note">没有注册成 Windows 服务的 MySQL 或 Redis。</p>';
  const rows = services.map(service => {
    const running = service.state === 'running' || service.state === 'start_pending';
    return `<div class="xp-row">
      <div class="xp-name"><strong>${escape(service.displayName || service.name)}</strong><span>${escape(service.binaryPath || service.name)}</span></div>
      <button type="button" class="xp-state ${running ? 'is-on' : 'is-off'}" data-service="${escape(service.name)}" data-service-action="${running ? 'stop' : 'start'}">${running ? '启动' : '停止'}</button>
      <span class="xp-port">—</span>
      <div class="xp-ops"><button type="button" data-service="${escape(service.name)}" data-service-action="restart">重启</button></div>
    </div>`;
  }).join('');
  return `<div class="xp-table" aria-label="Windows 服务"><div class="xp-head"><span>软件名称</span><span>状态</span><span>端口</span><span>操作</span></div>${rows}</div>`;
}

function renderPage() {
  local.innerHTML = softwareTable(programs);
  const dialog = byId('xd-infra-settings');
  const current = programs.find(item => item.kind === settingsKind);
  if (!dialog) return;
  if (!current) {
    if (dialog.open) dialog.close();
    return;
  }
  dialog.innerHTML = settingsPanel(current, settingsTab);
  if (!dialog.open) dialog.showModal();
}

function paintHome() {
  const host = document.getElementById('xd-home-db');
  if (host) host.innerHTML = homeBlock(programs);
}

function setPage(open) {
  pageOpen = open;
  page.hidden = !open;
  document.body.classList.toggle('xd-infra-open', open);
  document.getElementById('xd-infra-navbtn')?.classList.toggle('active', open);
  if (!open) {
    settingsKind = '';
    const dialog = byId('xd-infra-settings');
    if (dialog?.open) dialog.close();
  }
}

function ensureNav() {
  const nav = document.querySelector('.sidebar-nav');
  if (!nav || document.getElementById('xd-infra-nav')) return;
  const system = [...nav.querySelectorAll('.nav-section-label')].find(node => node.textContent.trim() === '系统工具');
  const section = document.createElement('div');
  section.className = 'nav-section';
  section.id = 'xd-infra-nav';
  section.innerHTML = `<div class="nav-section-label">数据库</div><button type="button" class="nav-item" id="xd-infra-navbtn"><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><ellipse cx="12" cy="6" rx="7" ry="3"/><path d="M5 6v6c0 1.7 3.1 3 7 3s7-1.3 7-3V6"/><path d="M5 12v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6"/></svg><span>MySQL / Redis</span></button>`;
  if (system?.parentElement) system.parentElement.before(section);
  else nav.append(section);
  if (pageOpen) section.querySelector('#xd-infra-navbtn')?.classList.add('active');
}

function ensureHome() {
  if (routePath(location) !== '/') {
    document.getElementById('xd-home-db')?.remove();
    return;
  }
  if (document.getElementById('xd-home-db')) return;
  const title = [...document.querySelectorAll('h2.page-title')].find(node => node.textContent.trim() === '系统工具');
  if (!title) return;
  const host = document.createElement('section');
  host.id = 'xd-home-db';
  host.className = 'xd-home-db';
  title.before(host);
  paintHome();
}

function readSettings(section) {
  const value = name => section.querySelector(`[data-field="${name}"]`)?.value;
  const saved = programs.find(item => item.kind === section.dataset.kind) || {};
  const read = name => {
    const field = value(name);
    return field === undefined ? (saved[name] ?? '') : field;
  };
  return {
    kind: section.dataset.kind,
    executable: read('executable'),
    port: Number(read('port') || saved.port),
    dataDir: read('dataDir'),
    configFile: read('configFile'),
    password: read('password'),
  };
}

async function refresh() {
  loading = true;
  status.textContent = '正在读取配置和服务…';
  try {
    const [nextPrograms, services] = await Promise.all([
      ipc('get_local_databases'),
      ipc('list_infra_services'),
    ]);
    programs = nextPrograms;
    renderPage();
    paintHome();
    const extra = serviceListState(programs, services);
    const title = byId('xd-infra-services-title');
    if (title) title.hidden = !extra.visible;
    list.hidden = !extra.visible;
    list.innerHTML = extra.visible ? serviceRows(extra.services) : '';
    status.textContent = '';
  } catch (error) {
    status.textContent = `读取失败：${error.message || error}`;
  } finally {
    loading = false;
  }
}

function openPage(kind = '') {
  settingsKind = kind;
  settingsTab = kind ? settingsTabs(kind)[0] : '';
  setPage(true);
  renderPage();
}

document.addEventListener('click', async event => {
  if (event.target.closest('#xd-infra-navbtn')) {
    openPage(settingsKind);
    return;
  }
  if (event.target.closest('.sidebar-nav .nav-item') && !event.target.closest('#xd-infra-navbtn')) {
    setPage(false);
  }
  const scope = event.target.closest('#xd-home-db, #xd-infra-page, #xd-infra-settings');
  if (!scope) return;
  const pick = event.target.closest('button[data-pick]');
  if (pick) {
    const section = pick.closest('.xp-settings');
    const input = section?.querySelector(`[data-field="${pick.dataset.pick}"]`);
    if (!section || !input || loading) return;
    try {
      const chosen = selectedPath(await ipc('plugin:dialog|open', {
        options: pickerOptions(section.dataset.kind, pick.dataset.pick, input.value),
      }));
      if (chosen) {
        input.value = chosen;
        if (pick.dataset.pick === 'executable') {
          const found = await ipc('locate_database_files', { kind: section.dataset.kind, executable: chosen });
          const item = programs.find(entry => entry.kind === section.dataset.kind);
          if (item) {
            item.executable = chosen;
            item.executableSource = 'custom';
            item.resolvedExecutable = chosen;
            if (found.configFile) item.configFile = found.configFile;
            if (found.dataDir) item.dataDir = found.dataDir;
          }
          renderPage();
          status.textContent = found.message || '已根据所选程序查找配置文件和数据目录。';
        }
      }
    } catch (error) {
      status.textContent = `无法打开选择窗口：${error.message || error}`;
    }
    return;
  }
  const tab = event.target.closest('button[data-tab]');
  if (tab) {
    settingsTab = tab.dataset.tab;
    renderPage();
    return;
  }
  const button = event.target.closest('button[data-action], button[data-local]');
  if (!button || loading) return;
  const kind = button.dataset.name || button.closest('[data-kind]')?.dataset.kind;
  const action = button.dataset.local || button.dataset.action;
  if (action === 'settings') {
    openPage(kind);
    return;
  }
  if (action === 'close-settings') {
    settingsKind = '';
    renderPage();
    return;
  }
  const title = kind === 'mysql' ? 'MySQL' : 'Redis';
  const running = programs.find(item => item.kind === kind)?.state === 'running';
  const verb = action === 'toggle' ? (running ? '停止' : '启动') : (action === 'save' ? '保存' : '重启');
  if ((verb === '停止' || action === 'restart') && !window.confirm(`确定${verb} ${title}？正在使用它的程序会断开连接。`)) return;
  loading = true;
  status.textContent = `正在${verb}…`;
  try {
    const control = verb === '启动' ? 'start' : verb === '停止' ? 'stop' : 'restart';
    const fields = button.closest('.xp-settings') ? readSettings(button.closest('.xp-settings')) : null;
    const result = action === 'save'
      ? await ipc('save_local_database', fields)
      : await ipc('control_local_database', { kind, action: control });
    const note = result.message || `${verb}完成。`;
    await refresh();
    status.textContent = note;
  } catch (error) {
    status.textContent = `操作失败：${error.message || error}`;
    loading = false;
  }
});

list.addEventListener('click', async event => {
  const button = event.target.closest('button[data-service-action]');
  if (!button || loading) return;
  const action = button.dataset.serviceAction;
  const name = button.dataset.service;
  const verb = { start: '启动', stop: '停止', restart: '重启' }[action];
  if (action !== 'start' && !window.confirm(`确定${verb} ${name}？正在使用它的程序会断开连接。`)) return;
  loading = true;
  status.textContent = `正在${verb} ${name}…`;
  try {
    const service = await ipc('control_infra_service', { name, action });
    const note = service.message || `${name} 现在是${service.stateLabel}。`;
    await refresh();
    status.textContent = note;
  } catch (error) {
    status.textContent = `操作失败：${error.message || error}`;
    loading = false;
  }
});

const settingsDialog = byId('xd-infra-settings');
settingsDialog.addEventListener('close', () => { settingsKind = ''; });
settingsDialog.addEventListener('click', event => { if (event.target === settingsDialog) settingsDialog.close(); });
byId('xd-infra-refresh').addEventListener('click', () => { if (!loading) refresh(); });
byId('xd-infra-close').addEventListener('click', () => setPage(false));
window.addEventListener('xd:routechange', () => { setPage(false); ensureHome(); });
window.addEventListener('hashchange', ensureHome);
new MutationObserver(() => { ensureNav(); ensureHome(); }).observe(document.body, { childList: true, subtree: true });
ensureNav();
ensureHome();

let attempts = 0;
function boot() {
  if (!window.__TAURI_INTERNALS__?.invoke) {
    if (++attempts < 200) setTimeout(boot, 50);
    return;
  }
  refresh();
}
boot();
