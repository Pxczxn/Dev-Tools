import { restartCurrentTerminal } from './log-restart.js';

window.xdRestartCurrentTerminal = (serviceId, actions = {}) => restartCurrentTerminal(serviceId, {
  unsubscribe: actions.unsubscribe,
  clear: actions.clear,
  subscribe: actions.subscribe,
  report: message => window.alert(message),
});
