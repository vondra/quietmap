//! Builds many squares at once in one process: the largest first, so that the longest start first
//! and the small ones fill the threads to the end; as many at a time as a memory budget admits,
//! a smaller square taking what a larger one waiting for memory leaves (a square needing more
//! than the whole budget runs once the others are done). Each square runs on a driver thread of
//! its own, outside the thread pool, and gives the pool only its parallel work inside: a pool
//! thread waiting for part of one square then takes part of another, never a whole square, which
//! would hold the first square (and its memory) until the second is done (on 2026-10-16 squares
//! spawned onto the pool itself nested 390 deep and left most of its threads asleep).

use crate::dev4::Square;
use std::sync::{Condvar, Mutex};

/// A square to build: the work its size stands for (any unit, compared between squares) and the
/// memory it may take at most (bytes).
pub struct Job {
    pub square: Square,
    pub work: u64,
    pub memory: u64,
}

/// The squares not yet started and the memory left.
struct Queue {
    jobs: Vec<Job>,
    memory: u64,
}

/// Builds every job's square with `build`, which returns the line to log; stops starting squares
/// after the first error and returns it once the running ones are done.
pub fn build_squares(
    mut jobs: Vec<Job>,
    budget: u64,
    build: impl Fn(Square) -> Result<String, String> + Sync,
) -> Result<(), String> {
    for job in &mut jobs {
        job.memory = job.memory.min(budget);
    }
    jobs.sort_by(|a, b| b.work.cmp(&a.work).then(a.square.cmp(&b.square)));
    let queue = Mutex::new(Queue {
        jobs,
        memory: budget,
    });
    let changed = Condvar::new();
    let failure: Mutex<Option<String>> = Mutex::new(None);
    // Twice the pool's threads: a small square's serial part (reading, the day loop) runs on its
    // driver while the others keep the pool busy.
    let drivers = 2 * rayon::current_num_threads();
    let next_job = || -> Option<Job> {
        let mut queue = queue.lock().expect("the queue");
        loop {
            if queue.jobs.is_empty() || failure.lock().expect("the failure").is_some() {
                return None;
            }
            let left = queue.memory;
            if let Some(index) = queue.jobs.iter().position(|job| job.memory <= left) {
                let job = queue.jobs.remove(index);
                queue.memory -= job.memory;
                return Some(job);
            }
            queue = changed.wait(queue).expect("the queue");
        }
    };
    std::thread::scope(|scope| {
        for _ in 0..drivers {
            scope.spawn(|| {
                while let Some(job) = next_job() {
                    match build(job.square) {
                        Ok(line) => eprintln!("{line}"),
                        Err(error) => {
                            failure.lock().expect("the failure").get_or_insert(error);
                        }
                    }
                    queue.lock().expect("the queue").memory += job.memory;
                    changed.notify_all();
                }
            });
        }
    });
    match failure.into_inner().expect("the failure") {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
