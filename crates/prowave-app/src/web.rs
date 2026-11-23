// SPDX-License-Identifier: GPL-3.0-or-later
//! Pro Wave in the browser, for Playground (patches/logic-pro.yaml in github.com/daijro/openplayground): the desktop
//! window on the page's canvas, its files in Playground's Files, and the browser's audio.
//!
//! `main` runs as on the desktop: the crate root takes [`eframe`] from here on the web, whose `run_native` builds the
//! app with the desktop's own creator, then starts it on the page. Files live in memory (prowave-playground's
//! `std::fs`): a project opened from Files comes in at its path (`browser:/Music/Song.prowave` is `/Music/Song.prowave`
//! here), File > Open… and Save As… are Playground's dialogs ([`rfd`]), and whatever Pro Wave writes outside its own
//! folders (a saved project) goes back to Files after the frame. "Open in Pro-Wave" opens a project, or imports an
//! audio file as a new track, as a file dropped on the window does. The engine plays through cpal's Web Audio host on
//! the page's one thread (index.html starts the audio at the first click or key).

use crate::App;
use ::eframe::egui;
use prowave_playground::std::fs;
use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};
use wasm_bindgen::{closure::Closure, JsCast as _, JsValue};

mod workspace;
pub(crate) use workspace::{queue_workspace_save, workspace, workspace_opened, workspace_save_pending};

/// Pro Wave's own folders in memory, never copied to Files: extracted media, settings, sessions' media.
const OWN: [&str; 3] = ["/tmp", "/home", "/live"];
/// Where dropped files are kept.
const DROPS: &str = "/Pro Wave/Dropped";

/// A dialog the app asked for this frame (see [`rfd`]).
#[derive(Clone, Copy, PartialEq)]
enum Ask {
    Open,
    Save { copy: bool },
}

thread_local! {
    static CTX: OnceCell<egui::Context> = const { OnceCell::new() };
    static ASKED: Cell<Option<Ask>> = const { Cell::new(None) };
    /// Files from Playground (Open, "Open in Pro-Wave"), for the next frame.
    static OPENED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    /// Where Save As… put the project (and whether as a copy), for the next frame.
    static SAVED: RefCell<Option<(PathBuf, bool)>> = const { RefCell::new(None) };
    /// Dropped files whose bytes are in memory now, for the next frame.
    static DROPPED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    static DROP_ID: Cell<u64> = const { Cell::new(0) };
}

pub fn repaint() {
    CTX.with(|c| c.get().map(egui::Context::request_repaint));
}

/// The page's egui context, once the app runs.
pub fn ctx() -> Option<egui::Context> {
    CTX.with(|c| c.get().cloned())
}

/// The desktop's eframe, with `run_native` starting the app on the page instead.
pub mod eframe {
    pub use ::eframe::*;

    /// The desktop window's options, which a page has no use for.
    #[derive(Default)]
    #[allow(dead_code)]
    pub struct NativeOptions {
        pub viewport: egui::ViewportBuilder,
        pub wgpu_options: egui_wgpu::WgpuConfiguration,
    }

    /// Builds the app with `creator` now (it borrows from `main`), then runs it on the page's canvas.
    pub fn run_native(_name: &str, _options: NativeOptions, creator: super::Creator<'_>) -> ::eframe::Result {
        super::start(creator)
    }

    /// The window's icon, which a page takes from its own `<link rel="icon">`.
    pub mod icon_data {
        pub fn from_png_bytes(_png: &[u8]) -> Result<::eframe::egui::IconData, String> {
            Ok(::eframe::egui::IconData::default())
        }
    }
}

/// The desktop's file dialogs: on the web they ask Playground's after the frame ([`Web::after`]) and give nothing now.
pub mod rfd {
    use std::path::PathBuf;

    pub struct FileDialog {
        title: String,
    }

    impl FileDialog {
        pub fn new() -> Self {
            Self { title: String::new() }
        }
        pub fn add_filter<T: ToString>(self, _name: impl Into<String>, _extensions: &[T]) -> Self {
            self
        }
        pub fn set_title(mut self, title: impl Into<String>) -> Self {
            self.title = title.into();
            self
        }
        pub fn set_file_name(self, _name: impl Into<String>) -> Self {
            self
        }
        pub fn pick_file(self) -> Option<PathBuf> {
            super::ASKED.set(Some(super::Ask::Open));
            None
        }
        pub fn pick_folder(self) -> Option<PathBuf> {
            None
        }
        pub fn save_file(self) -> Option<PathBuf> {
            if super::workspace() {
                super::call("status", &["This file action requires the desktop app.".into()]);
                return None;
            }
            super::ASKED.set(Some(super::Ask::Save { copy: self.title.contains("Copy") }));
            None
        }
    }
}

fn get(o: &JsValue, k: &str) -> JsValue {
    js_sys::Reflect::get(o, &k.into()).unwrap_or(JsValue::UNDEFINED)
}

/// `playgroundFiles[method](...args)`; None when the page has none or it threw.
fn call(method: &str, args: &[JsValue]) -> Option<JsValue> {
    let api = Some(get(&js_sys::global(), "playgroundFiles")).filter(JsValue::is_object)?;
    let f = get(&api, method).dyn_into::<js_sys::Function>().ok()?;
    f.apply(&api, &args.iter().collect::<js_sys::Array>()).ok()
}

async fn wait(v: Option<JsValue>) -> Option<JsValue> {
    let promise = v?.dyn_into::<js_sys::Promise>().ok()?;
    wasm_bindgen_futures::JsFuture::from(promise).await.ok().filter(|v| !v.is_null() && !v.is_undefined())
}

fn is_project(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(prowave_project::EXTENSION))
}

/// A file from Files (`{ path, name, bytes }`) into memory at its path; one from the computer goes to
/// `/Pro Wave/Opened`.
fn bring_in(file: &JsValue) -> Option<PathBuf> {
    let name = get(file, "name").as_string()?;
    let path = match get(file, "path").as_string().and_then(|p| p.strip_prefix("browser:").map(PathBuf::from)) {
        Some(path) => path,
        None => Path::new("/Pro Wave/Opened").join(Path::new(&name).file_name()?),
    };
    fs::put(&path, js_sys::Uint8Array::new(&get(file, "bytes")).to_vec());
    Some(path)
}

fn opened(file: JsValue) {
    if let Some(path) = bring_in(&file) {
        OPENED.with_borrow_mut(|o| o.push(path));
        repaint();
    }
}

fn types() -> JsValue {
    js_sys::Array::of1(&prowave_project::EXTENSION.into()).into()
}

/// The page's app: the window, then what it asked of the page.
pub struct Web {
    pub app: App,
    unsaved: Option<bool>,
    last_revision: u64,
}

impl Web {
    fn new(app: App) -> Self {
        let last_revision = app.project.song_rev;
        Self { app, unsaved: None, last_revision }
    }

    /// After the window's frame: files that came in, the dialogs it asked for, what it wrote, its unsaved state.
    fn after(&mut self, ctx: &egui::Context) {
        // What the frame spawned (waveform overviews), now that it is drawn.
        prowave_playground::std::thread::run_spawned();
        let completed = workspace::poll(&mut self.app, ctx);
        let incoming = if workspace_save_pending() { Vec::new() } else { OPENED.take() };
        let opened_any = !incoming.is_empty();
        for path in incoming {
            if is_project(&path) {
                // Through File > Open Recent's command, which asks about unsaved changes first.
                self.app.ui.recent_pick = Some(path);
                self.app.run_file_cmd(crate::file_commands::FileCmd::OpenRecent, ctx);
            } else {
                self.app.import(&path, ctx);
            }
        }
        if let Some((path, copy)) = SAVED.take() {
            if copy {
                self.app.save_copy_to(&path);
            } else {
                self.app.save_to(&path);
            }
        }
        match ASKED.take() {
            Some(Ask::Open) => {
                let options = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&options, &"types".into(), &types());
                let asked = call("open", &[options.into()]);
                wasm_bindgen_futures::spawn_local(async move {
                    if let Some(file) = wait(asked).await.and_then(|p| js_sys::Array::from(&p).iter().next()) {
                        opened(file);
                    }
                });
            }
            Some(Ask::Save { copy }) => self.save_as(copy),
            None => {}
        }
        let changes = fs::changes();
        for p in changes.into_iter().filter(|p| !workspace() && !OWN.iter().any(|own| p.starts_with(own))) {
            let target = JsValue::from(format!("browser:{}", p.display()));
            match fs::metadata(&p) {
                Ok(m) if m.is_file() => {
                    call("write", &[target, js_sys::Uint8Array::from(fs::read(&p).unwrap_or_default().as_slice()).into()]);
                }
                Ok(_) => {}
                Err(_) => {
                    call("remove", &[target]);
                }
            }
        }
        // ponytail: the project compared each frame something happened; a change counter if large projects stutter.
        let current = self.app.project.song_rev;
        let changed = self.last_revision != current;
        self.last_revision = current;
        if completed || opened_any || changed || ctx.input(|i| !i.events.is_empty()) || self.unsaved.is_none() {
            let unsaved = crate::file_commands::content(&self.app.project) != self.app.saved;
            if self.unsaved.replace(unsaved) != Some(unsaved) {
                call("setUnsaved", &[unsaved.into()]);
            }
        }
    }

    /// Save As… / Save A Copy As…: Playground's dialog, with the project as it is now for "Download to device"; a
    /// place in Files saves there on the next frame.
    fn save_as(&self, copy: bool) {
        let name = format!("{}.{}", self.app.project.name, prowave_project::EXTENSION);
        let scratch = Path::new("/tmp/prowave-save").join(&name);
        let bytes = crate::project_io::save_project(&self.app.project, &scratch).ok().and_then(|p| fs::read(p).ok());
        let options = js_sys::Object::new();
        for (k, v) in [
            ("name", JsValue::from(name.as_str())),
            ("types", types()),
            ("bytes", js_sys::Uint8Array::from(bytes.unwrap_or_default().as_slice()).into()),
        ] {
            let _ = js_sys::Reflect::set(&options, &k.into(), &v);
        }
        let asked = call("saveAs", &[options.into()]);
        wasm_bindgen_futures::spawn_local(async move {
            let path = wait(asked).await.and_then(|a| get(&a, "path").as_string());
            if let Some(path) = path.and_then(|p| p.strip_prefix("browser:").map(PathBuf::from)) {
                SAVED.set(Some((path, copy)));
                repaint();
            }
        });
    }
}

/// A dropped file, in memory.
#[derive(Debug)]
struct InMemory(PathBuf);

impl egui::DroppedFile for InMemory {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes_async(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + '_>> {
        let bytes = fs::read(&self.0).map_err(|e| e.to_string());
        Box::pin(async move { bytes })
    }
}

impl ::eframe::App for Web {
    fn raw_input_hook(&mut self, _: &egui::Context, raw_input: &mut egui::RawInput) {
        // A dropped file is only a name on the web: once its bytes are in memory at /Pro Wave/Dropped/<name>, a later
        // frame drops that file instead, as the desktop window gets it.
        raw_input.dropped_files.extend(DROPPED.take().into_iter().map(|p| std::sync::Arc::new(InMemory(p)) as _));
        for f in std::mem::take(&mut raw_input.dropped_files).into_iter() {
            if f.path().starts_with(DROPS) {
                raw_input.dropped_files.push(f);
                continue;
            }
            wasm_bindgen_futures::spawn_local(async move {
                if let (Ok(bytes), Some(name)) = (f.bytes_async().await, f.path().file_name()) {
                    let id = DROP_ID.get();
                    DROP_ID.set(id.wrapping_add(1));
                    let path = Path::new(DROPS).join(id.to_string()).join(name);
                    fs::put(&path, bytes);
                    DROPPED.with_borrow_mut(|d| d.push(path));
                    repaint();
                }
            });
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut ::eframe::Frame) {
        ::eframe::App::ui(&mut self.app, ui, frame);
        self.after(&ui.ctx().clone());
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// The page's web runner.
pub fn runner() -> ::eframe::WebRunner {
    thread_local! {
        static RUNNER: OnceCell<::eframe::WebRunner> = const { OnceCell::new() };
    }
    RUNNER.with(|kept| kept.get_or_init(::eframe::WebRunner::new).clone())
}

/// What the page runs: the app with the page's side around it.
fn wrap(app: App) -> Box<dyn ::eframe::App> {
    Box::new(Web::new(app))
}

/// The desktop's app creator, as `main` hands it over (a `Box<App>`, not eframe's `dyn App`).
pub type Creator<'a> = Box<dyn FnOnce(&::eframe::CreationContext<'_>) -> Result<Box<App>, DynError> + 'a>;
type DynError = Box<dyn std::error::Error + Send + Sync>;

/// [`eframe::run_native`] on the web: the app made now, by the desktop's creator against a scratch context whose
/// settings (look, zoom keys) then go to the page's.
fn start(creator: Creator<'_>) -> ::eframe::Result {
    std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&format!("Pro Wave: {info}").into())));
    let scratch = egui::Context::default();
    let app = *creator(&::eframe::CreationContext::_new_kittest(scratch.clone())).map_err(::eframe::Error::AppCreation)?;
    call("onOpen", &[Closure::<dyn Fn(JsValue)>::new(opened).into_js_value()]);
    wasm_bindgen_futures::spawn_local(async move {
        let document = web_sys::window().and_then(|w| w.document()).expect("no document");
        let canvas = document
            .get_element_by_id("prowave_canvas")
            .and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("no #prowave_canvas");
        let creator = move |cc: &::eframe::CreationContext<'_>| {
            cc.egui_ctx.options_mut(|o| *o = scratch.options(Clone::clone));
            CTX.with(|c| c.set(cc.egui_ctx.clone()).ok());
            Ok(wrap(app))
        };
        let started = runner().start(canvas, ::eframe::WebOptions::default(), Box::new(creator)).await;
        if let Some(loading) = document.get_element_by_id("prowave_loading") {
            match started {
                Ok(()) => {
                    loading.remove();
                    call("ready", &[]);
                }
                Err(e) => {
                    let text = format!("Pro Wave could not start: {e:?}");
                    loading.set_text_content(Some(&text));
                    call("failed", &[text.into()]);
                }
            }
        }
    });
    Ok(())
}
