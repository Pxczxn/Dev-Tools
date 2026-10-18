import { ipc } from './panel.js';
import { routePath } from './model.js';

const byId = id => document.getElementById(id);
const dialog = byId('xd-launch-dialog');
const serviceSelect = byId('xd-launch-service');
const optionSelect = byId('xd-launch-option');
const status = byId('xd-launch-status');
let services = [];
let options = [];
let generation = 0;
function option(label, value) { const node = document.createElement('option'); node.textContent = label; node.value = value; return node; }
function fill(service) {
  for (const [field, value] of Object.entries({ cwd: service.cwd, command: service.command, args: JSON.stringify(service.args || [], null, 2), prepare: JSON.stringify(service.prepare || [], null, 2), envfile: service.envFile || '', env: JSON.stringify(service.env || {}, null, 2) })) byId(`xd-launch-${field}`).value = value;
  byId('xd-launch-shell').checked = !!service.shellMode;
}
async function selectService() {
  const id = ++generation;
  const service = services.find(s => s.id === serviceSelect.value);
  if (!service) return;
  fill(service); options = [];
  optionSelect.replaceChildren(option('保留当前配置 / 自定义', 'custom'));
  status.textContent = '正在读取项目启动方式…';
  byId('xd-launch-description').textContent = '';
  try {
    const plans = await ipc('get_launch_options', { path: service.cwd });
    if (id !== generation) return;
    options = plans;
    plans.forEach((p, i) => optionSelect.append(option(p.label, String(i))));
    status.textContent = plans.length ? `发现 ${plans.length} 种启动方式。当前配置保留，选择建议后再保存。` : '未找到可确认的启动方式，请根据项目文档手动填写。';
  } catch (error) { if (id === generation) status.textContent = `读取失败：${error.message || error}`; }
}
optionSelect.addEventListener('change', () => {
  const service = services.find(s => s.id === serviceSelect.value);
  if (optionSelect.value === 'custom') { fill(service); byId('xd-launch-description').textContent = '保留当前服务设置，可手动修改。'; return; }
  const plan = options[Number(optionSelect.value)];
  if (!plan) return;
  fill({ ...service, ...plan });
  byId('xd-launch-description').textContent = plan.description;
});
serviceSelect.addEventListener('change', selectService);
byId('xd-launch-open').addEventListener('click', async () => {
  dialog.showModal(); status.textContent = '正在加载服务…';
  byId('xd-launch-save').disabled = true;
  const id = ++generation;
  try {
    const match = routePath(location).match(/^\/projects\/([^/]+)/);
    services = await ipc('list_services', match ? { workspaceId: decodeURIComponent(match[1]) } : {});
    if (id !== generation || !dialog.open) return;
    serviceSelect.replaceChildren(...services.map(s => option(s.name, s.id)));
    if (!services.length) { status.textContent = '请先在工作区添加或扫描服务。'; return; }
    byId('xd-launch-save').disabled = false;
    await selectService();
  } catch (error) { status.textContent = `加载失败：${error.message || error}`; }
});
byId('xd-launch-close').addEventListener('click', () => dialog.close());
dialog.addEventListener('click', event => { if (event.target === dialog) { const r = dialog.getBoundingClientRect(); if (event.clientX < r.left || event.clientX > r.right || event.clientY < r.top || event.clientY > r.bottom) dialog.close(); } });
dialog.addEventListener('close', () => { generation++; byId('xd-launch-open').focus(); });
byId('xd-launch-form').addEventListener('submit', async event => {
  event.preventDefault();
  const save = byId('xd-launch-save'); save.disabled = true;
  try {
    const parse = (field, label) => { try { return JSON.parse(byId(`xd-launch-${field}`).value); } catch { throw new Error(`${label}不是有效的 JSON，请检查引号、逗号和括号`); } };
    const args = parse('args', '启动参数');
    const prepare = parse('prepare', '前置步骤');
    const env = parse('env', '环境变量');
    if (!Array.isArray(args) || !args.every(a => typeof a === 'string')) throw new Error('启动参数必须为字符串数组');
    if (!Array.isArray(prepare) || !prepare.every(p => p && typeof p.cwd === 'string' && typeof p.command === 'string' && Array.isArray(p.args) && p.args.every(a => typeof a === 'string'))) throw new Error('前置步骤必须包含 cwd、command 和字符串 args 数组');
    if (!env || Array.isArray(env) || typeof env !== 'object' || !Object.values(env).every(v => typeof v === 'string')) throw new Error('环境变量必须为字符串键值对象');
    await ipc('update_service', { input: { id: serviceSelect.value, cwd: byId('xd-launch-cwd').value, command: byId('xd-launch-command').value, args, prepare, env, envFile: byId('xd-launch-envfile').value.trim() || null, shellMode: byId('xd-launch-shell').checked } });
    location.reload();
  } catch (error) { status.textContent = `保存失败：${error.message || error}`; save.disabled = false; }
});
