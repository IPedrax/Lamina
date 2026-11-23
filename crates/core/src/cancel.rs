//! Stopping long work part way. A front end that runs a command on a worker thread (the desktop's slicing and booleans) wraps it in
//! [`watch`] with a flag it can set from its own thread; the long loops (each layer of a slice, the wait for a boolean's child) ask
//! [`check`] and stop with [`CANCELLED`]. Work run without a flag never stops this way.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The error a cancelled command returns.
pub const CANCELLED: &str = "cancelled";

thread_local! {
    static FLAG: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Runs `f` on this thread with `flag` as its cancel flag: once it is set, [`check`] fails inside `f`. The flag before is put back.
pub fn watch<T>(flag: Arc<AtomicBool>, f: impl FnOnce() -> T) -> T {
    let before = FLAG.with(|c| c.borrow_mut().replace(flag));
    let out = f();
    FLAG.with(|c| *c.borrow_mut() = before);
    out
}

/// Whether the work on this thread was asked to stop.
pub fn requested() -> bool {
    FLAG.with(|c| c.borrow().as_ref().is_some_and(|f| f.load(Ordering::Relaxed)))
}

/// An error once the work on this thread was asked to stop, for `?` in a long loop.
pub fn check() -> Result<(), String> {
    if requested() { Err(CANCELLED.into()) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_flag_stops_only_the_work_it_watches() {
        assert!(check().is_ok(), "no flag, no stop");
        let flag = Arc::new(AtomicBool::new(false));
        let seen = watch(Arc::clone(&flag), || {
            let before = check();
            flag.store(true, Ordering::Relaxed);
            (before, check(), std::thread::spawn(requested).join().unwrap())
        });
        assert_eq!(seen, (Ok(()), Err(CANCELLED.to_string()), false), "another thread is not stopped");
        assert!(check().is_ok(), "the flag is gone after watch");
    }
}
