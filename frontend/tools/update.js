const OFFLINE = '暂时无法连接更新服务器，当前版本仍可离线使用';

function invoke(command, args = {}) {
  const bridge = window.__TAURI_INTERNALS__;
  if (!bridge?.invoke) return Promise.reject(new Error(OFFLINE));
  return bridge.invoke(command, args);
}

function cardFrom(title) {
  return title?.closest?.('.card') || title?.parentElement || null;
}

async function mount(card) {
  if (!card || card.querySelector('.xd-update')) return;
  const versionLine = [...card.querySelectorAll('.card-meta')].find(node => node.textContent.startsWith('Dev-Tools v'));
  try {
    const version = await invoke('get_app_version');
    if (versionLine && version) versionLine.textContent = `Dev-Tools v${version}`;
  } catch { /* 离线打开设置时保留页面上的版本文字。 */ }
  const box = document.createElement('div');
  box.className = 'xd-update';
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'btn btn-secondary';
  button.textContent = '检查更新';
  const status = document.createElement('p');
  status.className = 'xd-update-status';
  const notes = document.createElement('pre');
  notes.className = 'xd-update-notes';
  notes.hidden = true;
  box.append(button, status, notes);
  card.append(box);
  button.addEventListener('click', () => run(button, status, notes));
}

async function run(button, status, notes) {
  button.disabled = true;
  notes.hidden = true;
  if (button.dataset.phase === 'ready') {
    status.textContent = '正在下载并安装…';
    try {
      await invoke('update_install');
      status.textContent = '正在重新启动…';
    } catch (error) {
      status.textContent = error?.message || error || '更新失败，当前版本未改变';
      button.disabled = false;
    }
    return;
  }
  status.textContent = '正在检查更新…';
  try {
    const report = await invoke('update_check');
    if (report?.status === 'available') {
      button.textContent = '下载并安装';
      button.dataset.phase = 'ready';
      status.textContent = `发现新版本 ${report.latest}`;
      notes.hidden = !report.notes;
      notes.textContent = report.notes || '';
    } else {
      button.textContent = '检查更新';
      button.dataset.phase = '';
      status.textContent = report?.message || OFFLINE;
    }
  } catch (error) {
    button.textContent = '检查更新';
    status.textContent = error?.message || OFFLINE;
  } finally {
    if (button.dataset.phase !== 'ready' || status.textContent.startsWith('发现新版本')) button.disabled = false;
  }
}

function scan() {
  for (const title of document.querySelectorAll('.card-title')) {
    if (title.textContent.trim() === '关于') mount(cardFrom(title));
  }
}

new MutationObserver(scan).observe(document.body, { childList: true, subtree: true });
scan();
