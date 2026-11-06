// S'exécute dans la page, avant ses scripts : c'est le seul endroit où l'on peut intercepter son audio.
// Applique le volume de l'onglet et signale le micro et les appels. Ne lit ni n'envoie aucun son.
(() => {
  if (window.__melangeurReady) return;
  Object.defineProperty(window, '__melangeurReady', { value: true });

  let tabVolume = 1;
  const volumeOf = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'volume');
  const pageVolume = new WeakMap();   // Le volume voulu par la page, qu'elle continue de lire.
  const routedToWebAudio = new WeakSet(); // Déjà réglé par le gain Web Audio : ne pas appliquer deux fois.
  const elements = new Set();
  const masters = new Map();

  function applyTo(element) {
    if (routedToWebAudio.has(element)) return;
    const wanted = pageVolume.has(element) ? pageVolume.get(element) : volumeOf.get.call(element);
    pageVolume.set(element, wanted);
    volumeOf.set.call(element, Math.min(1, Math.max(0, wanted * tabVolume)));
  }

  function register(element) {
    if (element instanceof HTMLMediaElement && !elements.has(element)) {
      elements.add(element);
      applyTo(element);
    }
  }

  function applyAll() {
    for (const element of elements) {
      if (!element.isConnected && element.paused) { elements.delete(element); continue; }
      applyTo(element);
    }
    for (const [context, gain] of masters) {
      if (context.state === 'closed') { masters.delete(context); continue; }
      gain.gain.value = tabVolume;
    }
  }

  // --- Balises audio et vidéo, y compris celles jamais insérées dans la page ---
  Object.defineProperty(HTMLMediaElement.prototype, 'volume', {
    configurable: true,
    enumerable: true,
    get() { return pageVolume.has(this) ? pageVolume.get(this) : volumeOf.get.call(this); },
    set(value) {
      volumeOf.set.call(this, value); // Laisse le navigateur valider la valeur.
      pageVolume.set(this, value);
      register(this);
      applyTo(this);
    },
  });
  const play = HTMLMediaElement.prototype.play;
  HTMLMediaElement.prototype.play = function (...args) {
    register(this);
    return play.apply(this, args);
  };
  const srcObject = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'srcObject');
  Object.defineProperty(HTMLMediaElement.prototype, 'srcObject', {
    configurable: true,
    enumerable: true,
    get() { return srcObject.get.call(this); },
    set(value) { register(this); srcObject.set.call(this, value); },
  });
  // Les événements média ne remontent pas, mais la phase de capture les voit.
  for (const type of ['play', 'playing', 'loadedmetadata']) {
    document.addEventListener(type, (event) => register(event.target), true);
  }

  // --- Web Audio : tout ce qui va vers la sortie passe par un gain de l'onglet ---
  function masterOf(context) {
    let gain = masters.get(context);
    if (!gain) {
      gain = new GainNode(context, { gain: tabVolume });
      connect.call(gain, context.destination);
      masters.set(context, gain);
    }
    return gain;
  }
  const connect = AudioNode.prototype.connect;
  AudioNode.prototype.connect = function (target, ...rest) {
    if (target instanceof AudioDestinationNode && target.context instanceof AudioContext) {
      connect.call(this, masterOf(target.context), ...rest);
      return target;
    }
    return connect.call(this, target, ...rest);
  };
  const disconnect = AudioNode.prototype.disconnect;
  AudioNode.prototype.disconnect = function (target, ...rest) {
    if (target instanceof AudioDestinationNode && masters.has(target.context)) {
      return disconnect.call(this, masters.get(target.context), ...rest);
    }
    return disconnect.call(this, target, ...rest);
  };
  const createSource = AudioContext.prototype.createMediaElementSource;
  if (createSource) {
    AudioContext.prototype.createMediaElementSource = function (element) {
      volumeOf.set.call(element, pageVolume.has(element) ? pageVolume.get(element) : volumeOf.get.call(element));
      routedToWebAudio.add(element);
      return createSource.call(this, element);
    };
  }

  // --- Micro et appels : détectés même si le navigateur n'affiche aucune mention sur l'onglet ---
  const micTracks = new Set();
  const calls = new Set();
  let lastReport = '';

  function report() {
    for (const track of micTracks) if (track.readyState === 'ended') micTracks.delete(track);
    for (const call of calls) {
      if (call.connectionState === 'closed' || call.connectionState === 'failed') calls.delete(call);
    }
    const mic = micTracks.size > 0;
    const call = [...calls].some((c) => c.connectionState === 'connected' && c.melangeurReceivesAudio);
    const text = `${mic}|${call}`;
    if (text !== lastReport) {
      lastReport = text;
      window.dispatchEvent(new CustomEvent('__melangeur_state', { detail: { mic, call } }));
    }
  }

  const devices = navigator.mediaDevices;
  if (devices && devices.getUserMedia) {
    const getUserMedia = devices.getUserMedia;
    devices.getUserMedia = async function (...args) {
      const stream = await getUserMedia.apply(this, args);
      for (const track of stream.getAudioTracks()) {
        micTracks.add(track);
        track.addEventListener('ended', report);
      }
      report();
      return stream;
    };
    const stop = MediaStreamTrack.prototype.stop;
    MediaStreamTrack.prototype.stop = function () {
      stop.call(this);
      if (micTracks.delete(this)) report();
    };
  }

  const NativeConnection = window.RTCPeerConnection;
  if (NativeConnection) {
    window.RTCPeerConnection = new Proxy(NativeConnection, {
      construct(target, args, newTarget) {
        const connection = Reflect.construct(target, args, newTarget);
        calls.add(connection);
        connection.addEventListener('track', (event) => {
          if (event.track.kind === 'audio') {
            connection.melangeurReceivesAudio = true;
            // Certains sites jouent la voix via un élément créé plus tard : on le rattrape à play().
            report();
          }
        });
        connection.addEventListener('connectionstatechange', report);
        return connection;
      },
    });
  }

  window.addEventListener('__melangeur_set', (event) => {
    tabVolume = Math.min(1, Math.max(0, Number(event.detail.volume)));
    applyAll();
  });
  // Le pont peut démarrer après nous, ou l'extension avoir été rechargée.
  window.addEventListener('__melangeur_resend', () => {
    lastReport = '';
    report();
  });
})();
