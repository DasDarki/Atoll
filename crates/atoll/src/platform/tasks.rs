use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::thread;

type Completion = Box<dyn FnOnce(Box<dyn Any + Send>)>;
type Update = Box<dyn FnMut(Box<dyn Any + Send>)>;

thread_local! {
    static PENDING: RefCell<HashMap<u64, Completion>> = RefCell::default();
    static UPDATES: RefCell<HashMap<u64, Update>> = RefCell::default();
    static NEXT: Cell<u64> = const { Cell::new(0) };
}

fn next_id() -> u64 {
    NEXT.with(|next| {
        next.set(next.get() + 1);
        next.get()
    })
}

/// Runs `work` on its own thread and hands the result to `done` on the main thread, which is the
/// thread this is called on.
pub fn run_in_background<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let id = next_id();
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

/// Like [`run_in_background`], but `work` can also report updates while it runs. They reach
/// `update` on the main thread in the order they were sent, all before `done`.
pub fn run_with_updates<U: Send + 'static, T: Send + 'static>(
    name: &str,
    work: impl FnOnce(&dyn Fn(U)) -> T + Send + 'static,
    mut update: impl FnMut(U) + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let id = next_id();
    let receiver: Update = Box::new(move |value| {
        if let Ok(value) = value.downcast::<U>() {
            update(*value);
        }
    });
    UPDATES.with_borrow_mut(|updates| updates.insert(id, receiver));
    let send = move |value: U| {
        let value: Box<dyn Any + Send> = Box::new(value);
        let _ = slint::invoke_from_event_loop(move || {
            let receiver = UPDATES.with_borrow_mut(|updates| updates.remove(&id));
            if let Some(mut receiver) = receiver {
                receiver(value);
                UPDATES.with_borrow_mut(|updates| updates.insert(id, receiver));
            }
        });
    };
    run_in_background(
        name,
        move || work(&send),
        move |result| {
            UPDATES.with_borrow_mut(|updates| updates.remove(&id));
            done(result);
        },
    );
}
