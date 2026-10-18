const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
// Isolated browser interaction fixture. No command execution or application data writes.
const root = path.resolve(__dirname, '../frontend');
const fixture = JSON.parse(fs.readFileSync(path.resolve(__dirname, '../src-tauri/target/workspace-smoke.json'), 'utf8'));
const plan = fixture.backendOptions[0];
const service = { id: 'preview-backend', workspaceId: 'preview', name: '星语后端（交互测试）', ...plan, type: 'java', enabled: true, env: {} };
const bootstrap = `
const callbacks = new Map(); let next = 1;
const service = ${JSON.stringify(service).replace(/</g, '\\u003c')};
const options = ${JSON.stringify(fixture.backendOptions).replace(/</g, '\\u003c')};
window.__TAURI_INTERNALS__ = {
 transformCallback(fn) { const id = next++; callbacks.set(id, fn); return id; },
 unregisterCallback(id) { callbacks.delete(id); },
 async invoke(cmd, args = {}) {
   if (cmd === 'list_services') return [service];
   if (cmd === 'get_launch_options') return options;
   if (cmd === 'update_service') { Object.assign(service, args.input); return service; }
   if (cmd === 'plugin:event|listen') {
     if (args.event === 'log:batch') setTimeout(() => callbacks.get(args.handler)?.({ payload: { serviceId: service.id, entries: [
       { text: '\\u001b[31mPort 7779 was already in use\\u001b[0m', timestamp: new Date().toISOString() },
       { text: '[ERROR] custom <unsafe> failure', timestamp: new Date().toISOString() },
       { text: 'Port 7779 was already in use', timestamp: new Date().toISOString() }
     ] } }), 200);
     return next++;
   }
   if (cmd === 'query_multiple_ports') return [{port: args.ports[0], protocol: 'tcp', pid:1234}];
   if (cmd === 'get_process_info') return [{name:'node.exe', pid:1234, memoryUsage:10485760, path:'D:\\\\node.exe'}];
   if (cmd === 'get_settings') return { theme:'dark', startupInterval:500, maxLogLines:5000, minimizeToTray:false };
   if (cmd === 'get_app_version') return 'preview';
   if (cmd === 'health_check') return {app_version:'preview', ipc_status:'ONLINE'};
   if (cmd.startsWith('list_') || cmd === 'get_process_runtime') return [];
   if (cmd === 'plugin:event|unlisten') return;
   throw new Error('交互测试：该命令未模拟 ' + cmd);
 }
};
`;
const types = { '.js':'text/javascript', '.css':'text/css', '.html':'text/html', '.svg':'image/svg+xml' };
http.createServer((req, res) => {
  const requested = path.resolve(root, '.' + decodeURIComponent(new URL(req.url, 'http://localhost').pathname));
  if (!requested.startsWith(root + path.sep) && requested !== root) { res.writeHead(403); return res.end(); }
  const file = requested === root ? path.join(root, 'index.html') : requested;
  try {
    let content = fs.readFileSync(file);
    if (path.basename(file) === 'index.html') content = content.toString().replace('<head>', '<head><script>' + bootstrap + '</script>');
    res.setHeader('Content-Type', (types[path.extname(file)] || 'application/octet-stream') + '; charset=utf-8'); res.end(content);
  } catch { res.writeHead(404); res.end(); }
}).listen(4180, '127.0.0.1', () => console.log('Isolated interaction fixture http://127.0.0.1:4180; IPC is simulated.'));
