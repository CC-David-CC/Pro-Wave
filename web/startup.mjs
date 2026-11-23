// SPDX-License-Identifier: GPL-3.0-or-later

const files = window.frameElement?.proWaveFiles;
if (files) {
  window.playgroundFiles = {
    ...files,
    open: async (...args) => files.open(...args),
    saveAs: async (...args) => files.saveAs(...args),
    persist: async (...args) => files.persist(...args),
  };
}

const contexts = new Set();
if (window.AudioContext) {
  window.AudioContext = class extends window.AudioContext {
    constructor(...options) {
      super(...options);
      contexts.add(this);
    }
  };
}

function resumeAudio() {
  for (const audio of contexts) {
    if (audio.state === 'suspended') audio.resume().catch(() => {});
  }
}

for (const type of ['pointerdown', 'keydown', 'touchend']) {
  addEventListener(type, resumeAudio, {capture: true});
}
addEventListener('pagehide', () => {
  for (const audio of contexts) {
    if (audio.state !== 'closed') audio.close().catch(() => {});
  }
  contexts.clear();
});

try {
  const bindings = await import('./prowave.js');
  window.wasmBindings = bindings;
  const wasm = await bindings.default();
  dispatchEvent(new CustomEvent('TrunkApplicationStarted', {detail: {wasm}}));
} catch (error) {
  const message = 'Pro Wave could not start. ' + (error?.message || String(error));
  const loading = document.getElementById('prowave_loading');
  if (loading) loading.textContent = message;
  files?.failed(message);
}
