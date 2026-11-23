# Pro Wave in WebOS

The browser build uses Pro Wave at demo revision `ae7921fa1fa4dfd08bf14b3659ff9e006d4fca9d` and the standalone OpenPlayground port at revision `4317d6b7ec991926713fd1af1bc715c42c0e8a55`. It runs the upstream egui editor and Rust audio engine in WebAssembly.

Build from the repository root with Rust 1.99.1, the `wasm32-unknown-unknown` target and Trunk 0.21.14. Set `CC_wasm32_unknown_unknown` to clang with WebAssembly support if the host's C compiler cannot build that target.

```sh
trunk build --release --locked
```

Package the output under `native/`, `web/mount.mjs` as `index.mjs` and `web/workspace.css` beside the mount. Ship the GPL license, applicable library notices and access to the corresponding source for the distributed WASM.

File → Open reads `.prowave` projects and imports WAV, AIFF and FLAC audio from My files. The native `.prowave` archive contains the editable project and its referenced audio. Save As can keep a separate copy when another editor changes the file. Audio starts after user input in the editor. Native plug-in hosting and desktop file dialogs are outside the browser build.
