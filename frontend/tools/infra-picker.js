export function pickerOptions(kind, field, current) {
  const defaultPath = String(current || '').trim();
  const options = { multiple: false };
  if (defaultPath) options.defaultPath = defaultPath;
  if (field === 'dataDir') {
    return { ...options, title: '选择数据目录', directory: true };
  }
  if (field === 'configFile') {
    return {
      ...options,
      title: '选择配置文件',
      filters: [
        { name: '配置文件', extensions: ['ini', 'cnf', 'conf'] },
        { name: '所有文件', extensions: ['*'] },
      ],
    };
  }
  const executable = kind === 'mysql' ? 'mysqld.exe' : 'redis-server.exe';
  return {
    ...options,
    title: `选择 ${executable}`,
    filters: [
      { name: executable, extensions: ['exe'] },
      { name: '所有文件', extensions: ['*'] },
    ],
  };
}

export function selectedPath(result) {
  return typeof result === 'string' && result.trim() ? result : '';
}
