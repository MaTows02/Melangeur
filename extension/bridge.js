// Pont entre la page (sans accès aux API de l'extension) et le service worker.
(() => {
  if (window.__melangeurBridge) return;
  window.__melangeurBridge = true;

  const setVolume = (volume) => window.dispatchEvent(new CustomEvent('__melangeur_set', { detail: { volume } }));

  window.addEventListener('__melangeur_state', (event) => {
    const { mic, call } = event.detail;
    chrome.runtime.sendMessage({ type: 'state', mic: Boolean(mic), call: Boolean(call) }).catch(() => {});
  });

  chrome.runtime.onMessage.addListener((message) => {
    if (message.type === 'volume') setVolume(message.volume);
  });

  // Une nouvelle page dans un onglet déjà réglé reprend son volume.
  chrome.runtime.sendMessage({ type: 'hello' }).then((reply) => {
    if (reply && reply.volume !== 1) setVolume(reply.volume);
  }).catch(() => {});
  window.dispatchEvent(new CustomEvent('__melangeur_resend'));
})();
