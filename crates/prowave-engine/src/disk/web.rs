// SPDX-License-Identifier: GPL-3.0-or-later
//! Disk streaming in the browser (patches/logic-pro.yaml in github.com/daijro/openplayground): a page cannot start the
//! streaming thread, so the clips are read in turns instead, each time the engine renders a block ([`pump`], from
//! `EngineCore::process`, on the page's one thread). Same work as the thread's loop, without its waiting.

use super::{handle, DiskThread, Msg, Progress, Stream};
use std::cell::RefCell;
use std::sync::mpsc;

thread_local! {
    /// The clips being streamed and where new ones arrive.
    static WORK: RefCell<Option<(mpsc::Receiver<Msg>, Vec<Stream>)>> = const { RefCell::new(None) };
}

/// [`DiskThread::spawn`] on the web: the streams are read by [`pump`].
pub(super) fn spawn() -> DiskThread {
    let (tx, rx) = mpsc::channel();
    WORK.set(Some((rx, Vec::new())));
    DiskThread { tx, handle: None, load: Default::default() }
}

/// One turn of the streaming loop: new clips registered, every clip's ring topped up.
pub fn pump() {
    WORK.with_borrow_mut(|work| {
        let Some((rx, streams)) = work else { return };
        while let Ok(msg) = rx.try_recv() {
            if !handle(msg, streams) {
                *work = None;
                return;
            }
        }
        streams.retain_mut(|s| !matches!(s.service(), Progress::Gone));
    });
}
