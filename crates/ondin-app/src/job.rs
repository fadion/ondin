//! Work the UI thread hands off: one computation on a worker thread, whose answer
//! the frame loop picks up when it lands. The worker asks for a repaint as it
//! finishes, so an idle window still sees the answer.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// One computation on a worker, answering once.
pub(crate) struct Job<T> {
    rx: Receiver<T>,
}

impl<T: Send + 'static> Job<T> {
    /// Run `work` on a thread named `name`. A job whose thread could not start
    /// answers [`Self::poll`] with `Err`.
    pub(crate) fn spawn(
        name: &str,
        ctx: &egui::Context,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Self {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let _ = tx.send(work());
                ctx.request_repaint();
            });
        if let Err(e) = spawned {
            log::warn!(target: "ondin", "no thread for {name}: {e}");
        }
        Self { rx }
    }

    /// The answer, once it has landed; `Err(())` if the worker died without one.
    pub(crate) fn poll(&self) -> Result<Option<T>, ()> {
        match self.rx.try_recv() {
            Ok(v) => Ok(Some(v)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(()),
        }
    }

    /// Block until the answer lands; `None` if the worker died without one.
    pub(crate) fn wait(self) -> Option<T> {
        self.rx.recv().ok()
    }
}
