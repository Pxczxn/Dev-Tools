const NOISE = /^\s*\[ERROR\]\s*(?:$|To see the full stack trace|Re-run Maven|For more information|\[Help \d+\] https?:|https?:\/\/cwiki\.apache\.org)|^\s*\[INFO\]\s*-{5,}|^\s*\[\d+m/;
      // 错误特征 → 中文提示（正则, 提示；$1..$9 引用捕获组），特定规则在前
  const RULES = [
        [/Unable to rename .*\.jar/i, "打包产物无法重命名：检查是否被运行中的 Java 进程占用，以及目录写入权限；依赖已安装时可选择直接启动"],
        [/前置构建失败/, "前置构建失败：已取消启动，请先处理构建日志中的具体错误"],
        [/Port (\d+) was already in use/, "端口 $1 已被占用：到「端口占用」查询并终止占用进程，或修改应用端口"],
        [/EADDRINUSE/, "监听端口被占用：先释放端口或换端口"],
        [/APPLICATION FAILED TO START/, "Spring 启动失败：原因见相邻 Description/Action 段落"],
        [/Failed to execute goal (\S+) .*exit code: (\d+)/, "构建目标 $1 执行失败（退出码 $2）：查看上方 Spring 异常或编译错误"],
        [/No plugin found for prefix '([^']+)'/, "Maven 插件前缀 $1 无法解析：多模块工程需在启动模块目录执行，或使用全限定目标"],
        [/BUILD FAILURE/, "Maven 构建失败：定位上方 ERROR 行"],
        [/Connection refused|ECONNREFUSED/, "连接被拒绝：目标服务（数据库/Redis 等）未启动或端口不对"],
        [/Access denied for user/, "数据库账号或密码错误"],
        [/Communications link failure|UnknownHostException/, "数据库主机/端口无法连接"],
        [/UnsupportedClassVersionError/, "Java 版本不匹配：编译版本高于运行版本"],
        [/Could not find or load main class/, "主类未找到：先完成编译（mvn compile / gradle build）"],
        [/OutOfMemoryError|heap space/i, "内存不足：调大 JVM 堆内存或检查内存泄漏"],
        [/Cannot find module '([^']+)'/, "Node 依赖 $1 缺失：按项目指定的包管理器安装依赖"],
        [/error TS\d+/, "TypeScript 编译错误：按错误码定位源码"],
        [/Caused by: ([\w.$]+(?:Exception|Error))/, "出现异常 $1：根因看最后一条 Caused by"],
      ];
  function cleanText(text) {
    return text.replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, '')
      .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '').trim().slice(0, 16000);
  }
  function hintFor(text) {
    if (NOISE.test(text)) return null;
    for (const [pattern, message] of RULES) {
      const match = text.match(pattern);
      if (match) return message.replace(/\$(\d)/g, (_, n) => match[Number(n)] || '');
    }
    if (/(?:\bERROR\b|\bFATAL\b|\b\w*(?:Exception|Error):|Traceback \(most recent call last\))/i.test(text)) {
      return '日志报告错误：展开原文，并结合相邻日志定位原因';
    }
    return null;
  }
  function createCollector() {
    const items = [];
    return {
      items,
      clear() { items.length = 0; },
      addBatch(payload) {
        if (!payload || !Array.isArray(payload.entries)) return false;
        let changed = false;
        const serviceId = String(payload.serviceId || '未知服务');
        for (const entry of payload.entries) {
          if (!entry || typeof entry.text !== 'string') continue;
          const text = cleanText(entry.text);
          const hint = hintFor(text);
          if (!hint) continue;
          const index = items.findIndex(item => item.serviceId === serviceId && item.text === text);
          const item = index < 0 ? { serviceId, text, hint, count: 0 } : items.splice(index, 1)[0];
          item.count++;
          item.lastAt = typeof entry.timestamp === 'string' ? entry.timestamp : '';
          items.unshift(item);
          if (items.length > 60) items.length = 60;
          changed = true;
        }
        return changed;
      },
    };
  }
  function parseQuery(input) {
    let value = input.trim();
    if (value.startsWith('"') && value.endsWith('"')) value = value.slice(1, -1).trim();
    if (!value) return { kind: 'empty', value: '' };
    if (/^\d+$/.test(value)) {
      const port = Number(value);
      return port >= 1 && port <= 65535 ? { kind: 'port', value: port } : { kind: 'invalid', value };
    }
    if (/^[a-z]:[\\/]/i.test(value) || value.startsWith('\\\\')) return { kind: 'path', value };
    return { kind: 'process', value };
  }
function routePath(location) { return (location.hash?.startsWith('#/') ? location.hash.slice(1) : location.pathname).split('?')[0]; }
export { createCollector, parseQuery, routePath };
