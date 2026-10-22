//! Background work for the UI: run a closure on a worker thread and deliver its result as a
//! message on the next frame (the worker wakes the UI via `request_repaint`).

use std::sync::mpsc::{channel, Receiver, Sender};

pub struct Jobs<M: Send + 'static> {
    tx: Sender<M>,
    rx: Receiver<M>,
    ctx: egui::Context,
}

impl<M: Send + 'static> Jobs<M> {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        Self { tx, rx, ctx }
    }

    pub fn spawn(&self, name: &str, f: impl FnOnce() -> M + Send + 'static) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let res = std::thread::Builder::new().name(name.to_string()).spawn(move || {
            let msg = f();
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
        if let Err(e) = res {
            log::error!("cannot spawn worker {name}: {e}");
        }
    }

    /// A sender for long-running workers that report more than once.
    pub fn sender(&self) -> (Sender<M>, egui::Context) {
        (self.tx.clone(), self.ctx.clone())
    }

    pub fn drain(&self) -> Vec<M> {
        self.rx.try_iter().collect()
    }
}
