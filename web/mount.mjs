// SPDX-License-Identifier: GPL-3.0-or-later

const revision = /^[a-f0-9]{64}$/;
const invalidName = /[/\\:\u0000-\u001f]/;
const projectExtension = '.prowave';
const fileTypes = [projectExtension, '.wav', '.wave', '.aif', '.aiff', '.aifc', '.flac'];
const maxBytes = 128 * 1024 * 1024;

function element(tag, properties) {
  return Object.assign(document.createElement(tag), properties);
}

function basename(path) {
  return path.split('/').at(-1);
}

function workspacePath(path) {
  if (typeof path !== 'string') throw Error('Choose a file in My files.');
  const relative = path.replace(/^(?:browser:)?\//, '');
  if (relative.split('/').some(part => !part || part === '.' || part === '..' || invalidName.test(part))) {
    throw Error('Choose a file in My files.');
  }
  return relative;
}

export function mount(host) {
  const owner = host.storage.workspaceOwner();
  const session = host.session();
  const controller = new AbortController();
  const {signal} = controller;
  const root = element('section', {className: 'window-content prowave-workspace'});
  const view = element('iframe', {title: 'Pro Wave', allow: 'autoplay'});
  const status = element('p', {textContent: 'Starting Pro Wave…'});
  status.setAttribute('role', 'status');
  root.append(element('link', {rel: 'stylesheet', href: new URL('./workspace.css', import.meta.url).href}), view, status);

  const opened = new Map();
  const loadedRevisions = new Map();
  let context, pending, openFile, unregister, binding;
  let dirty = false;
  let transferring = false;
  let startResolve, startReject;
  const start = new Promise((resolve, reject) => {
    startResolve = resolve;
    startReject = reject;
  });
  const timer = setTimeout(() => startReject(Error('Pro Wave took too long to start.')), 90000);
  signal.addEventListener('abort', () => startReject(signal.reason), {once: true});
  start.catch(() => {});

  function check() {
    if (signal.aborted || host.session() !== session || host.storage.workspaceOwner() !== owner || !context?.valid()) {
      throw new DOMException('This Files session ended.', 'AbortError');
    }
    if (transferring) throw Error('Files are being transferred.');
  }

  function run(operation) {
    if (pending) return Promise.reject(Error('Pro Wave is waiting for Files.'));
    pending = Promise.resolve().then(() => {
      check();
      return operation();
    });
    return pending.catch(error => {
      if (!signal.aborted) status.textContent = error.message;
      throw error;
    }).finally(() => { pending = undefined; });
  }

  async function read(path) {
    check();
    const loaded = await context.files.read(path, {signal});
    check();
    if (!revision.test(loaded.sha256)) throw Error('Files returned an invalid revision.');
    if (loaded.data.byteLength > maxBytes) throw Error('Pro Wave can open files up to 128 MiB.');
    if (path.toLowerCase().endsWith(projectExtension)) opened.set(path, loaded.sha256);
    return {name: basename(path), path: 'browser:/' + path, bytes: loaded.data, sha256: loaded.sha256};
  }

  function bind(path, sha256, action) {
    binding = path ? {path, sha256} : undefined;
    host.desktop.updateAccountFileBinding(host.w, {owner, path, sha256});
    status.textContent = action && path ? action + ' ' + basename(path) : 'Pro Wave is ready';
  }

  view.proWaveFiles = {
    workspace: true,
    onOpen(callback) { openFile = callback; },
    ready() { startResolve(); },
    failed(message) { startReject(Error(message)); },
    setUnsaved(value) { dirty = value; },
    status(message) { status.textContent = message; },
    open() {
      return run(async () => {
        const choice = await host.storage.pickWorkspaceFile({extensions: fileTypes, signal});
        return choice ? [await read(workspacePath(choice))] : null;
      });
    },
    opened(path) {
      check();
      const source = path == null ? null : workspacePath(path);
      const sha256 = opened.get(source) ?? loadedRevisions.get(source);
      opened.delete(source);
      if (sha256) {
        loadedRevisions.set(source, sha256);
        bind(source, sha256, 'Opened');
      } else bind(null, null);
    },
    saveAs({name}) {
      return run(async () => {
        const folder = await host.storage.pickWorkspaceFile({foldersOnly: true, signal});
        if (!folder) return null;
        const choice = await host.ui.prompt('File name', {title: 'Save Pro Wave project', value: name, confirmLabel: 'Save', signal});
        if (choice == null) return null;
        let filename = choice.trim();
        if (!filename || invalidName.test(filename) || filename === '.' || filename === '..') throw Error('Enter a file name.');
        if (!filename.endsWith(projectExtension)) {
          if (filename.toLowerCase().endsWith(projectExtension)) filename = filename.slice(0, -projectExtension.length);
          filename += projectExtension;
        }
        const path = workspacePath((folder === '/' ? '' : folder + '/') + filename);
        return {path: 'browser:/' + path};
      });
    },
    persist(path, bytes, copy) {
      return run(async () => {
        const target = workspacePath(path);
        if (!target.toLowerCase().endsWith(projectExtension)) throw Error('Choose a .prowave project file.');
        if (bytes.byteLength > maxBytes) throw Error('Pro Wave can save projects up to 128 MiB.');
        const expected = !copy && target === binding?.path ? binding.sha256 : null;
        let saved;
        try {
          saved = await context.files.write(target, new Blob([bytes], {type: 'application/zip'}), expected, {signal});
        } catch (error) {
          if (error?.status === 409) throw Error('This file already exists or changed. Save as a new file to keep both copies.');
          throw error;
        }
        check();
        if (!revision.test(saved.sha256)) throw Error('Files did not confirm the save.');
        if (copy) status.textContent = 'Saved a copy ' + basename(target);
        else {
          loadedRevisions.set(target, saved.sha256);
          bind(target, saved.sha256, 'Saved');
        }
        return true;
      });
    },
  };

  const ready = (async () => {
    context = await host.storage.workspaceFileContext();
    check();
    if (context.owner !== owner) throw Error('This editor belongs to another Files workspace.');
    view.src = new URL('./native/index.html', import.meta.url).href;
    await start;
    check();
    unregister = context.registerFileWriter?.(async () => {
      if (pending) await pending;
      if (dirty) throw Error('Save the Pro Wave project before transferring Files.');
      transferring = true;
      view.inert = true;
      return () => {
        transferring = false;
        view.inert = false;
      };
    });
    status.textContent = 'Pro Wave is ready';
    const launch = host.w.launchContext;
    if (launch) {
      if (launch.storage !== 'account' || launch.owner !== owner) throw Error('Open the project from My files.');
      const project = await read(workspacePath(launch.path));
      if (!launch.restore && launch.sha256 && launch.sha256 !== project.sha256) throw Error('The project changed in Files. Open its current revision.');
      openFile(project);
    }
  })().finally(() => clearTimeout(timer));
  ready.catch(error => { if (!signal.aborted) status.textContent = error.message; });

  return {
    element: root,
    ready,
    focus: () => view.focus(),
    inspect: () => ({path: binding?.path ?? null, dirty}),
    async requestClose() {
      if (pending) await pending.catch(() => {});
      return !dirty || host.ui.confirm('Discard unsaved changes to this project?', {confirmLabel: 'Discard', signal});
    },
    dispose() {
      controller.abort();
      unregister?.();
    },
  };
}
