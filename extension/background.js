// Service worker : suit les onglets sonores par événements et dialogue avec Melangeur.
// Seuls les onglets avec du son, un micro ou un appel sont transmis, et uniquement à Melangeur sur ce PC.

const HOST = 'com.melangeur.host';
const SEND_DELAY_MS = 100;
const RECONNECT_DELAY_MS = 5000;

const volumes = new Map();   // tabId -> volume de 0 à 1
const signals = new Map();   // tabId -> Map(frameId -> { mic, call })
let port = null;
let sendTimer = null;
let ready = null;

function restore() {
  ready ??= chrome.storage.session.get('volumes').then(({ volumes: saved }) => {
    for (const [id, value] of Object.entries(saved || {})) volumes.set(Number(id), value);
  });
  return ready;
}

function saveVolumes() {
  chrome.storage.session.set({ volumes: Object.fromEntries(volumes) });
}

function frameSignal(tabId) {
  const frames = signals.get(tabId);
  const result = { mic: false, call: false };
  if (frames) {
    for (const frame of frames.values()) {
      result.mic ||= frame.mic;
      result.call ||= frame.call;
    }
  }
  return result;
}

async function sendState() {
  sendTimer = null;
  if (!port) return;
  await restore();
  const list = [];
  for (const tab of await chrome.tabs.query({})) {
    const { mic, call } = frameSignal(tab.id);
    const volume = volumes.get(tab.id) ?? 1;
    const muted = Boolean(tab.mutedInfo && tab.mutedInfo.muted);
    // Un onglet baissé ou coupé reste listé, même silencieux : sinon on ne pourrait plus le remonter.
    if (!tab.audible && !mic && !call && volume === 1 && !muted) continue;
    list.push({
      id: tab.id,
      title: tab.title || '',
      audible: Boolean(tab.audible),
      muted,
      volume,
      mic,
      call,
    });
  }
  port?.postMessage({ tabs: list });
}

function scheduleSend() {
  sendTimer ??= setTimeout(sendState, SEND_DELAY_MS);
}

function connect() {
  if (port) return;
  try {
    port = chrome.runtime.connectNative(HOST);
  } catch {
    port = null;
    return;
  }
  port.onMessage.addListener(onCommand);
  port.onDisconnect.addListener(() => {
    void chrome.runtime.lastError; // Melangeur fermé ou hôte absent : on réessaiera.
    port = null;
    setTimeout(connect, RECONNECT_DELAY_MS);
  });
  scheduleSend();
}

async function onCommand(message) {
  await restore();
  switch (message.cmd) {
    case 'hello':
      scheduleSend();
      break;
    case 'volume': {
      const volume = Math.min(1, Math.max(0, Number(message.value)));
      volumes.set(message.tab, volume);
      saveVolumes();
      scheduleSend();
      // Envoyé à toutes les frames de l'onglet : le son d'un appel est souvent dans une sous-frame.
      chrome.tabs.sendMessage(message.tab, { type: 'volume', volume }).catch(() => {});
      break;
    }
    case 'mute':
      chrome.tabs.update(message.tab, { muted: Boolean(message.value) }).catch(() => {});
      break;
    case 'activate': {
      const tab = await chrome.tabs.update(message.tab, { active: true }).catch(() => null);
      if (tab) chrome.windows.update(tab.windowId, { focused: true }).catch(() => {});
      break;
    }
  }
}

chrome.runtime.onMessage.addListener((message, sender, reply) => {
  const tabId = sender.tab && sender.tab.id;
  if (tabId === undefined) return false;
  if (message.type === 'hello') {
    restore().then(() => reply({ volume: volumes.get(tabId) ?? 1 }));
    return true;
  }
  if (message.type === 'state') {
    if (!signals.has(tabId)) signals.set(tabId, new Map());
    signals.get(tabId).set(sender.frameId, { mic: message.mic, call: message.call });
    scheduleSend();
  }
  return false;
});

chrome.tabs.onUpdated.addListener((tabId, change) => {
  if (change.status === 'loading' && !change.url) return;
  if (change.status === 'loading') signals.delete(tabId); // Nouvelle page : anciens signaux périmés.
  if ('audible' in change || 'mutedInfo' in change || 'title' in change || change.status === 'loading') scheduleSend();
});

chrome.tabs.onRemoved.addListener((tabId) => {
  signals.delete(tabId);
  if (volumes.delete(tabId)) saveVolumes();
  scheduleSend();
});

// À l'installation, les pages déjà ouvertes n'ont pas encore les scripts.
chrome.runtime.onInstalled.addListener(async () => {
  for (const tab of await chrome.tabs.query({})) {
    if (!tab.url || !/^(https?|file):/.test(tab.url)) continue;
    const target = { tabId: tab.id, allFrames: true };
    chrome.scripting.executeScript({ target, files: ['page.js'], world: 'MAIN' }).catch(() => {});
    chrome.scripting.executeScript({ target, files: ['bridge.js'] }).catch(() => {});
  }
});

restore();
connect();
