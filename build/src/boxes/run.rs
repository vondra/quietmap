//! Builds many squares at once in one process: the largest first, so that the longest start first
//! and the small ones fill the threads to the end; as many at a time as a memory budget admits
//! (a square needing more than the whole budget runs once the others are done). Each square's
//! builder runs on the shared thread pool with its own parallel work inside.

use crate::dev4::Square;
use std::sync::{Condvar, Mutex};

/// A square to build: the work its size stands for (any unit, compared between squares) and the
/// memory it may take at most (bytes).
pub struct Job {
    pub square: Square,
    pub work: u64,
    pub memory: u64,
}

/// Builds every job's square with `build`, which returns the line to log; stops admitting squares
/// after the first error and returns it once the running ones are done.
pub fn build_squares(
    mut jobs: Vec<Job>,
    budget: u64,
    build: impl Fn(Square) -> Result<String, String> + Sync,
) -> Result<(), String> {
    jobs.sort_by(|a, b| b.work.cmp(&a.work).then(a.square.cmp(&b.square)));
    let free = Mutex::new(budget);
    let freed = Condvar::new();
    let failure: Mutex<Option<String>> = Mutex::new(None);
    rayon::in_place_scope(|scope| {
        for job in jobs {
            let need = job.memory.min(budget);
            let mut left = freed
                .wait_while(free.lock().expect("the budget"), |left| *left < need)
                .expect("the budget");
            if failure.lock().expect("the failure").is_some() {
                break;
            }
            *left -= need;
            drop(left);
            let (build, free, freed, failure) = (&build, &free, &freed, &failure);
            scope.spawn(move |_| {
                match build(job.square) {
                    Ok(line) => eprintln!("{line}"),
                    Err(error) => {
                        failure.lock().expect("the failure").get_or_insert(error);
                    }
                }
                *free.lock().expect("the budget") += need;
                freed.notify_all();
            });
        }
    });
    match failure.into_inner().expect("the failure") {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
