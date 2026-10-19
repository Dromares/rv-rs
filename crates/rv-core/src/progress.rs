//! Progress reporting and cancellation for the long operations (DAT update, scan).
//!
//! ROMVault passes a `ThreadWorker` around (`RomVaultCore/ThreadWorker.cs`): a global, shared by
//! the GUI and the command line. Here an operation takes a [`ProgressSink`] and a [`CancelFlag`]
//! explicitly and keeps no state of its own, so two operations on two databases can run side by
//! side.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// One progress message (the `bgw*` classes of `BackGroundWorkerInterface.cs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgressEvent {
    /// What the operation is doing ("Scanning Dir : ...", "Scanning New Dat : ...").
    Text(String),
    /// The item being worked on (`bgwText2`).
    Item(String),
    /// Detail for a long item, such as "Hashing: 42%" (`bgwText3`).
    Detail(String),
    /// The main bar: `done` of `total` (`bgwValue`, `bgwSetRange`).
    Overall { done: u64, total: u64 },
    /// The second bar, inside the current directory (`bgwValue2`, `bgwSetRange2`).
    Sub { done: u64, total: u64 },
    /// A file that could not be used. The operation goes on (`bgwShowError`).
    Error { file: String, message: String },
}

/// Receives [`ProgressEvent`]s. Closures work: `|e: ProgressEvent| println!("{e:?}")`.
pub trait ProgressSink {
    fn report(&self, event: ProgressEvent);
}

impl<F: Fn(ProgressEvent)> ProgressSink for F {
    fn report(&self, event: ProgressEvent) {
        self(event)
    }
}

/// A sink that drops everything.
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn report(&self, _event: ProgressEvent) {}
}

/// A shared flag that asks an operation to stop. Operations look at it between items (a DAT, a
/// directory, a name group), never inside one: a file that is being hashed is finished first,
/// and the database is consistent whenever an operation returns.
#[derive(Clone, Debug, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> CancelFlag {
        CancelFlag::default()
    }

    /// Ask for a stop. Clones of this flag see it.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn clones_share_the_flag() {
        let a = CancelFlag::new();
        let b = a.clone();
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(b.is_cancelled());
    }

    #[test]
    fn closures_are_sinks() {
        let seen = RefCell::new(Vec::new());
        let sink = |e: ProgressEvent| seen.borrow_mut().push(e);
        sink.report(ProgressEvent::Text("x".into()));
        NoProgress.report(ProgressEvent::Text("y".into()));
        assert_eq!(*seen.borrow(), [ProgressEvent::Text("x".into())]);
    }
}
