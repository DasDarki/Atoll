use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::thread;

type Completion = Box<dyn FnOnce(Box<dyn Any + Send>)>;

thread_local! {
    static PENDING: RefCell<HashMap<u64, Completion>> = RefCell::default();
    static NEXT: Cell<u64> = const { Cell::new(0) };
}

/// Runs `work` on its own thread and hands the result to `done` on the main thread, which is the
/// thread this is called on.
pub fn run_in_background<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let id = NEXT.with(|next| {
        next.set(next.get() + 1);
        next.get()
    });
    let completion: Completion = Box::new(move |result| {
        if let Ok(result) = result.downcast::<T>() {
            done(*result);
        }
    });
    PENDING.with_borrow_mut(|pending| pending.insert(id, completion));
    let spawned = thread::Builder::new().name(name.to_owned()).spawn(move || {
        let result: Box<dyn Any + Send> = Box::new(work());
        let _ = slint::invoke_from_event_loop(move || {
            let completion = PENDING.with_borrow_mut(|pending| pending.remove(&id));
            if let Some(completion) = completion {
                completion(result);
            }
        });
    });
    if let Err(error) = spawned {
        PENDING.with_borrow_mut(|pending| pending.remove(&id));
        tracing::error!(%error, name, "background task could not be started");
    }
}
