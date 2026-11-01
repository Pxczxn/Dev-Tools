const escape = value => String(value ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

export function settingsTabs(kind) {
  return kind === 'redis'
    ? ['性能调整', '配置文件', '当前状态']
    : ['端口', '存储位置', '配置文件', '当前状态'];
}

function stateButton(running, action, name) {
  const label = running ? '启动' : '停止';
  const tone = running ? 'is-on' : 'is-off';
  return `<button type="button" class="xp-state ${tone}" data-action="${action}" data-name="${escape(name)}" aria-pressed="${running}">${label}</button>`;
}

export function softwareTable(items) {
  const rows = items.map(item => {
    const running = item.state === 'running';
    const title = item.kind === 'mysql' ? 'MySQL' : 'Redis';
    const used = item.resolvedExecutable || item.executable;
    const detail = item.executableSource === 'default'
      ? `默认安装 · ${escape(used)}`
      : item.executableSource === 'custom'
        ? `自定义路径 · ${escape(used)}`
        : '尚未选择程序';
    return `<div class="xp-row" data-kind="${item.kind}">
      <div class="xp-name"><span class="xp-id"><strong>${title}</strong><span>${detail}</span></span></div>
      ${stateButton(running, 'toggle', item.kind)}
      <span class="xp-port">${Number(item.port) || ''}</span>
      <div class="xp-ops"><button type="button" data-action="settings" data-name="${item.kind}">设置</button><button type="button" data-action="restart" data-name="${item.kind}">重启</button></div>
    </div>`;
  }).join('');
  return `<div class="xp-table" aria-label="本机数据库">
    <div class="xp-head"><span>软件名称</span><span>状态</span><span>端口</span><span>操作</span></div>
    ${rows}
  </div>`;
}

function mysqlPathNote(item) {
  if (item.kind !== 'mysql') return '';
  if (item.executableSource === 'default') {
    return `<p class="xp-note">先使用默认安装：${escape(item.resolvedExecutable)}。默认安装不存在时，才使用上面的自定义路径。</p>`;
  }
  return '<p class="xp-note">未找到 C:\\Program Files\\MySQL 下的默认安装，将使用自定义路径。</p>';
}

function pathField(label, field, value, placeholder) {
  return `<label>${label}<span class="xd-infra-path"><input data-field="${field}" value="${escape(value)}" placeholder="${escape(placeholder)}" /><button type="button" data-pick="${field}">浏览</button></span></label>`;
}

export function settingsPanel(item, tab) {
  const tabs = settingsTabs(item.kind);
  const current = tabs.includes(tab) ? tab : tabs[0];
  const title = item.kind === 'mysql' ? 'MySQL' : 'Redis';
  const running = item.state === 'running';
  const nav = tabs.map(name => `<button type="button" class="xp-tab${name === current ? ' is-active' : ''}" data-tab="${name}">${name}</button>`).join('');
  let body = '';
  if (current === '端口' || current === '性能调整') {
    const password = item.kind === 'redis'
      ? `<label>requirepass 密码<input data-field="password" type="password" value="${escape(item.password || '')}" placeholder="留空表示不设置" autocomplete="off" /></label>`
      : '';
    const extras = item.kind === 'redis'
      ? `<dl class="xp-fixed"><div><dt>bind</dt><dd>127.0.0.1</dd></div><div><dt>timeout</dt><dd>0，空闲连接不断开</dd></div><div><dt>maxclients</dt><dd>不单独限制</dd></div><div><dt>databases</dt><dd>16</dd></div><div><dt>maxmemory</dt><dd>0，不限制</dd></div></dl>`
      : '';
    body = `<label>port 端口<input data-field="port" inputmode="numeric" value="${Number(item.port) || ''}" /></label>${password}${extras}`;
  } else if (current === '存储位置') {
    body = `${pathField('自定义程序路径', 'executable', item.executable, 'mysqld.exe')}
      ${mysqlPathNote(item)}
      ${pathField('数据目录', 'dataDir', item.dataDir, '数据文件所在目录')}`;
  } else if (current === '配置文件') {
    const program = item.kind === 'mysql' ? 'mysqld.exe' : 'redis-server.exe';
    body = `${pathField('程序路径', 'executable', item.executable, program)}
      ${item.kind === 'redis' ? pathField('数据目录', 'dataDir', item.dataDir, '数据文件所在目录') : ''}
      ${pathField('配置文件', 'configFile', item.configFile, '可留空。填写后按这个文件启动')}
      <p class="xp-note">这里是主配置文件路径。不了解规则时不要改内容，保存后需要重启才生效。</p>`;
  } else {
    body = `<dl class="xp-fixed"><div><dt>当前状态</dt><dd>${running ? '启动' : '停止'}</dd></div><div><dt>PID</dt><dd>${item.pid || '无'}</dd></div><div><dt>端口</dt><dd>${Number(item.port) || ''}</dd></div><div><dt>程序</dt><dd>${escape(item.executable || '未设置')}</dd></div></dl>`;
  }
  return `<section class="xp-settings" data-kind="${item.kind}" data-tab="${current}">
    <header><strong>${title} 设置</strong><span class="xp-settings-actions"><button type="button" data-local="save">保存</button><button type="button" data-action="close-settings">关闭</button></span></header>
    <nav>${nav}</nav>
    <div class="xp-settings-body">${body}</div>
  </section>`;
}

export function homeBlock(items) {
  return `<h2 class="page-title">数据库</h2>${softwareTable(items)}`;
}

function executablePath(value) {
  const trimmed = String(value ?? '').trim();
  if (trimmed.startsWith('"')) {
    const end = trimmed.indexOf('"', 1);
    return (end === -1 ? trimmed.slice(1) : trimmed.slice(1, end)).trim();
  }
  const lower = trimmed.toLowerCase();
  const index = lower.indexOf('.exe');
  if (index !== -1) {
    const end = index + 4;
    const next = trimmed[end];
    if (next === undefined || /\s/.test(next)) return trimmed.slice(0, end);
  }
  return trimmed.split(/\s+/)[0] || '';
}

export function serviceListState(programs, services) {
  const shown = new Set();
  for (const item of programs) {
    const path = executablePath(item.resolvedExecutable || item.executable).toLowerCase();
    if (path) shown.add(path);
  }
  const extra = services.filter(service => {
    const path = executablePath(service.binaryPath).toLowerCase();
    return !path || !shown.has(path);
  });
  return { visible: extra.length > 0, services: extra };
}
