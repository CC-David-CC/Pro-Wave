// SPDX-License-Identifier: GPL-3.0-or-later

use super::{call, fs, get, repaint, types, App};
use crate::file_commands::{content, FileCmd};
use prowave_project::ProjectDoc;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;

struct SaveRequest {
    path: Option<PathBuf>,
    copy: bool,
    then: Option<FileCmd>,
}

struct Saved {
    path: PathBuf,
    document: ProjectDoc,
}

struct Completion {
    request: SaveRequest,
    result: Result<Option<Saved>, String>,
}

thread_local! {
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static SAVE: RefCell<Option<SaveRequest>> = const { RefCell::new(None) };
    static COMPLETE: RefCell<Option<Completion>> = const { RefCell::new(None) };
}

pub(crate) fn workspace() -> bool {
    get(&get(&js_sys::global(), "playgroundFiles"), "workspace").as_bool() == Some(true)
}

pub(crate) fn workspace_save_pending() -> bool {
    BUSY.get()
}

pub(crate) fn queue_workspace_save(path: Option<PathBuf>, copy: bool, then: Option<FileCmd>) {
    if BUSY.replace(true) {
        return;
    }
    SAVE.set(Some(SaveRequest { path, copy, then }));
    repaint();
}

pub(crate) fn workspace_opened(path: Option<&Path>) {
    if workspace() {
        let value = path.map(virtual_path).unwrap_or(JsValue::NULL);
        call("opened", &[value]);
    }
}

fn virtual_path(path: &Path) -> JsValue {
    format!("browser:{}", path.display()).into()
}

async fn request(task: Option<JsValue>) -> Result<Option<JsValue>, String> {
    let promise = task.and_then(|value| value.dyn_into::<js_sys::Promise>().ok())
        .ok_or("Files is unavailable.")?;
    let result = wasm_bindgen_futures::JsFuture::from(promise).await.map_err(|error| {
        get(&error, "message").as_string().unwrap_or_else(|| "Files could not complete the operation.".to_string())
    })?;
    Ok(Some(result).filter(|value| !value.is_null() && !value.is_undefined()))
}

async fn choose(name: &str) -> Result<Option<PathBuf>, String> {
    let options = js_sys::Object::new();
    for (key, value) in [("name", name.into()), ("types", types())] {
        let _ = js_sys::Reflect::set(&options, &key.into(), &value);
    }
    let choice = request(call("saveAs", &[options.into()])).await?;
    Ok(choice.and_then(|value| get(&value, "path").as_string())
        .and_then(|path| path.strip_prefix("browser:").map(PathBuf::from)))
}

fn start(app: &App, save: SaveRequest) {
    let document = content(&app.project);
    let scratch = Path::new("/tmp/prowave-workspace-save/project.prowave");
    let bytes = crate::project_io::save_project(&app.project, scratch)
        .map_err(|error| error.to_string())
        .and_then(|path| fs::read(&path).map_err(|error| error.to_string()));
    let _ = fs::remove_file(scratch);
    let suffix = if save.copy { " copy" } else { "" };
    let name = format!("{}{suffix}.{}", app.project.name, prowave_project::EXTENSION);
    wasm_bindgen_futures::spawn_local(async move {
        let result = write(&save, name, bytes, document).await;
        COMPLETE.set(Some(Completion { request: save, result }));
        repaint();
    });
}

async fn write(
    save: &SaveRequest,
    name: String,
    bytes: Result<Vec<u8>, String>,
    document: ProjectDoc,
) -> Result<Option<Saved>, String> {
    let bytes = bytes?;
    let path = match &save.path {
        Some(path) => path.clone(),
        None => match choose(&name).await? {
            Some(path) => path,
            None => return Ok(None),
        },
    };
    let payload = js_sys::Uint8Array::from(bytes.as_slice());
    let receipt = request(call("persist", &[virtual_path(&path), payload.into(), save.copy.into()])).await?;
    if receipt.and_then(|value| value.as_bool()) != Some(true) {
        return Err("Files did not confirm the save.".to_string());
    }
    if !save.copy {
        fs::put(&path, bytes);
    }
    Ok(Some(Saved { path, document }))
}

pub(super) fn poll(app: &mut App, ctx: &super::egui::Context) -> bool {
    let completion = COMPLETE.take();
    let completed = completion.is_some();
    if let Some(Completion { request, result }) = completion {
        BUSY.set(false);
        match result {
            Ok(Some(saved)) => app.finish_workspace_save(saved.path, saved.document, request.copy, request.then, ctx),
            Ok(None) => {
                if request.then.is_some() {
                    app.ui.recent_pick = None;
                }
            }
            Err(error) => {
                if request.then.is_some() {
                    app.ui.recent_pick = None;
                }
                app.ui.status = Some(error);
            }
        }
    }
    if let Some(save) = SAVE.take() {
        start(app, save);
    }
    completed
}
