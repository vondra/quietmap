//! Builds many squares at once in one process: the largest first, so that the longest start first
//! and the small ones fill the threads to the end; as many at a time as a memory budget admits,
//! a smaller square taking what a larger one waiting for memory leaves (a square needing more
//! than the whole budget runs once the others are done). Each square runs on a driver thread of
//! its own, outside the thread pool, and gives the pool only its parallel work inside: a pool
//! thread waiting for part of one square then takes part of another, never a whole square, which
//! would hold the first square (and its memory) until the second is done (on 2026-10-16 squares
//! spawned onto the pool itself nested 390 deep and left most of its threads asleep).

use crate::dev4::Square;
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Condvar, Mutex};

/// A square to build: the work its size stands for (any unit, compared between squares) and the
/// memory it may take at most (bytes).
pub struct Job {
    pub square: Square,
    pub work: u64,
    pub memory: u64,
}

/// The squares not yet started, the memory left, and the first failure.
struct Queue {
    jobs: Vec<Job>,
    memory: u64,
    failure: Option<String>,
}

fn panic_message(square: Square, payload: Box<dyn Any + Send>) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic");
    format!("square {}/{}: panicked: {message}", square.x, square.y)
}

/// Builds every job's square with `build`, which returns the line to log; stops starting squares
/// after the first error or panic and returns it once the running ones are done.
pub fn build_squares(
    jobs: Vec<Job>,
    budget: u64,
    build: impl Fn(Square) -> Result<String, String> + Sync,
) -> Result<(), String> {
    build_squares_with_log(jobs, budget, build, |line| eprintln!("{line}"))
}

fn build_squares_with_log(
    mut jobs: Vec<Job>,
    budget: u64,
    build: impl Fn(Square) -> Result<String, String> + Sync,
    log: impl Fn(&str) + Sync,
) -> Result<(), String> {
    for job in &mut jobs {
        job.memory = job.memory.min(budget);
    }
    jobs.sort_by(|a, b| b.work.cmp(&a.work).then(a.square.cmp(&b.square)));
    let queue = Mutex::new(Queue {
        jobs,
        memory: budget,
        failure: None,
    });
    let changed = Condvar::new();
    // Twice the pool's threads: a small square's serial part (reading, the day loop) runs on its
    // driver while the others keep the pool busy.
    let drivers = 2 * rayon::current_num_threads();
    let next_job = || -> Option<Job> {
        let mut queue = queue.lock().expect("the queue");
        loop {
            if queue.jobs.is_empty() || queue.failure.is_some() {
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
                    let result = catch_unwind(AssertUnwindSafe(|| build(job.square)))
                        .unwrap_or_else(|payload| Err(panic_message(job.square, payload)));
                    // Return the reservation and wake waiting drivers before logging so that a
                    // panicking write (such as stderr on a full disk) cannot strand the queue.
                    let line = {
                        let mut queue = queue.lock().expect("the queue");
                        let line = match result {
                            Ok(line) => Some(line),
                            Err(error) => {
                                queue.failure.get_or_insert(error);
                                None
                            }
                        };
                        queue.memory += job.memory;
                        changed.notify_all();
                        line
                    };
                    if let Some(line) = line {
                        log(&line);
                    }
                }
            });
        }
    });
    let queue = queue.into_inner().expect("the queue");
    assert_eq!(queue.memory, budget);
    match queue.failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Barrier, mpsc};
    use std::time::Duration;

    fn job(x: u32, work: u64, memory: u64) -> Job {
        Job {
            square: Square { x, y: 0 },
            work,
            memory,
        }
    }

    fn run_within<T: Send + 'static>(
        deadline: Duration,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        rx.recv_timeout(deadline)
            .expect("build_squares did not finish before the deadline")
    }

    /// A driver that panics returns its memory, wakes waiting drivers and fails the step with the
    /// panic's message instead of leaving the remaining drivers waiting forever.
    #[test]
    fn a_panicking_square_fails_the_step_and_returns_its_memory() {
        let (error, ran) = run_within(Duration::from_secs(5), || {
            let both_running = Barrier::new(2);
            let ran = AtomicUsize::new(0);
            let jobs = vec![job(1, 30, 60), job(2, 20, 40), job(3, 10, 60)];
            let error = build_squares(jobs, 100, |square| {
                ran.fetch_add(1, Ordering::Relaxed);
                match square.x {
                    1 => {
                        both_running.wait();
                        panic!("installation mixes of a tile");
                    }
                    2 => {
                        both_running.wait();
                        Ok("square 2/0: ok".into())
                    }
                    other => Ok(format!("square {other}/0: ok")),
                }
            })
            .unwrap_err();
            (error, ran.load(Ordering::Relaxed))
        });
        assert!(error.contains("1/0"), "{error}");
        assert!(error.contains("installation mixes of a tile"), "{error}");
        assert_eq!(ran, 2);
    }

    /// A panicking log write after a finished square cannot strand a waiting square behind its
    /// memory reservation.
    #[test]
    fn a_panicking_log_line_does_not_strand_waiting_squares() {
        let (panicked, ran) = run_within(Duration::from_secs(5), || {
            let ran = AtomicUsize::new(0);
            let jobs = vec![job(1, 20, 100), job(2, 10, 1)];
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                build_squares_with_log(
                    jobs,
                    100,
                    |square| {
                        ran.fetch_add(1, Ordering::Relaxed);
                        Ok(format!("square {}/0: ok", square.x))
                    },
                    |line| {
                        if line.contains("1/0") {
                            panic!("no space left on device");
                        }
                    },
                )
            }));
            (outcome.is_err(), ran.load(Ordering::Relaxed))
        });
        assert!(panicked);
        assert_eq!(ran, 2);
    }
}
