export async function restartCurrentTerminal(serviceId, { unsubscribe, clear, subscribe, report } = {}) {
  const id = String(serviceId ?? '').trim();
  if (!id) return 'empty';
  try {
    if (unsubscribe) await unsubscribe(id);
    await clear(id);
    if (subscribe) await subscribe(id);
    return 'restarted';
  } catch (error) {
    const message = error && error.message ? error.message : String(error ?? '未知错误');
    (report ?? (() => {}))(`终端没能重新打开：${message}`);
    return 'failed';
  }
}
