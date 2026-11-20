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
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Condvar, Mutex, PoisonError};

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

/// One square's reserved memory: returns the reservation, records any failure and wakes waiting
/// drivers under one lock on every exit (normal return, error or unwind).
struct Reservation<'a> {
    queue: &'a Mutex<Queue>,
    changed: &'a Condvar,
    memory: u64,
    failure: Option<String>,
}

impl Reservation<'_> {
    fn fail(mut self, error: String) {
        self.failure = Some(error);
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(error) = self.failure.take() {
            queue.failure.get_or_insert(error);
        }
        queue.memory += self.memory;
        self.changed.notify_all();
    }
}

fn format_panic_message(square: Square, payload: &(dyn Any + Send)) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic");
    format!("square {}/{}: panicked: {message}", square.x, square.y)
}

fn run_log(
    square: Square,
    log: &(impl Fn(&str) -> Result<(), String> + Sync),
    line: &str,
) -> Result<(), String> {
    match catch_unwind(AssertUnwindSafe(|| log(line))) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(format!(
            "square {}/{}: log write failed: {error}",
            square.x, square.y
        )),
        Err(payload) => {
            let message = format_panic_message(square, &*payload);
            let _ = catch_unwind(AssertUnwindSafe(move || drop(payload)));
            Err(message)
        }
    }
}

/// Builds every job's square with `build`, which returns the line to log; stops starting squares
/// after the first error or panic and returns it once the running ones are done.
pub fn build_squares(
    jobs: Vec<Job>,
    budget: u64,
    build: impl Fn(Square) -> Result<String, String> + Sync,
) -> Result<(), String> {
    build_squares_with_log(jobs, budget, build, |line| {
        writeln!(std::io::stderr(), "{line}").map_err(|error| error.to_string())
    })
}

fn build_squares_with_log(
    mut jobs: Vec<Job>,
    budget: u64,
    build: impl Fn(Square) -> Result<String, String> + Sync,
    log: impl Fn(&str) -> Result<(), String> + Sync,
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
    let next_job = || -> Option<(Square, Reservation<'_>)> {
        let mut queue_guard = queue.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if queue_guard.jobs.is_empty() || queue_guard.failure.is_some() {
                return None;
            }
            let left = queue_guard.memory;
            if let Some(index) = queue_guard.jobs.iter().position(|job| job.memory <= left) {
                let job = queue_guard.jobs.remove(index);
                queue_guard.memory -= job.memory;
                return Some((
                    job.square,
                    Reservation {
                        queue: &queue,
                        changed: &changed,
                        memory: job.memory,
                        failure: None,
                    },
                ));
            }
            queue_guard = changed
                .wait(queue_guard)
                .unwrap_or_else(PoisonError::into_inner);
        }
    };
    std::thread::scope(|scope| {
        for _ in 0..drivers {
            scope.spawn(|| {
                while let Some((square, reservation)) = next_job() {
                    match catch_unwind(AssertUnwindSafe(|| build(square))) {
                        Ok(Ok(line)) => {
                            // Return the reservation and wake waiters before logging so a failed
                            // or slow log write cannot strand another square behind this budget.
                            drop(reservation);
                            if let Err(error) = run_log(square, &log, &line) {
                                let mut queue =
                                    queue.lock().unwrap_or_else(PoisonError::into_inner);
                                queue.failure.get_or_insert(error);
                                changed.notify_all();
                            }
                        }
                        Ok(Err(error)) => reservation.fail(error),
                        Err(payload) => {
                            // Extract the message and release the reservation before dropping the
                            // payload: a panicking payload destructor cannot strand the queue.
                            let message = format_panic_message(square, &*payload);
                            reservation.fail(message);
                            let _ = catch_unwind(AssertUnwindSafe(move || drop(payload)));
                        }
                    }
                }
            });
        }
    });
    let queue = queue.into_inner().unwrap_or_else(PoisonError::into_inner);
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

    /// A failing log write after a finished square returns an ordinary error and releases the
    /// square's memory first, so a waiting square is never stranded behind it.
    #[test]
    fn a_failing_log_line_returns_an_error_without_stranding_waiting_squares() {
        let (error, ran) = run_within(Duration::from_secs(5), || {
            let ran = AtomicUsize::new(0);
            let job2_started = Barrier::new(2);
            let jobs = vec![job(1, 20, 100), job(2, 10, 1)];
            let error = build_squares_with_log(
                jobs,
                100,
                |square| {
                    ran.fetch_add(1, Ordering::Relaxed);
                    if square.x == 2 {
                        job2_started.wait();
                    }
                    Ok(format!("square {}/0: ok", square.x))
                },
                |line| {
                    if line.contains("1/0") {
                        // Must run after square 1 returned its full-budget reservation, otherwise
                        // square 2 can never start and this blocks until the deadline.
                        job2_started.wait();
                        Err("no space left on device".into())
                    } else {
                        Ok(())
                    }
                },
            )
            .unwrap_err();
            (error, ran.load(Ordering::Relaxed))
        });
        assert!(error.contains("1/0"), "{error}");
        assert!(error.contains("no space left on device"), "{error}");
        assert_eq!(ran, 2);
    }

    /// A failing log line (even a panicking logger) beside an already recorded build error never
    /// unwinds out of `build_squares` and preserves the build's own diagnostic.
    #[test]
    fn a_failing_build_keeps_its_error_beside_a_failing_log() {
        let error = run_within(Duration::from_secs(5), || {
            let both_started = Barrier::new(2);
            let err_about_to_return = Barrier::new(2);
            let jobs = vec![job(1, 30, 50), job(2, 20, 50), job(3, 10, 50)];
            build_squares_with_log(
                jobs,
                100,
                |square| match square.x {
                    1 => {
                        both_started.wait();
                        Ok("square 1/0: ok".into())
                    }
                    2 => {
                        both_started.wait();
                        err_about_to_return.wait();
                        Err("square 2/0: corrupt input".into())
                    }
                    other => Ok(format!("square {other}/0: ok")),
                },
                |line| {
                    if line.contains("1/0") {
                        err_about_to_return.wait();
                        std::thread::sleep(Duration::from_millis(50));
                        panic!("stderr write panicked");
                    }
                    Ok(())
                },
            )
            .unwrap_err()
        });
        assert!(error.contains("square 2/0: corrupt input"), "{error}");
    }

    /// A build that unwinds with a `panic_any` payload whose own destructor panics still returns
    /// its full-budget reservation, wakes waiting drivers and fails with the square's error.
    #[test]
    fn a_panicking_payload_destructor_still_returns_its_memory() {
        struct DropPanics;
        impl Drop for DropPanics {
            fn drop(&mut self) {
                panic!("payload drop panicked");
            }
        }

        let (error, ran) = run_within(Duration::from_secs(5), || {
            let ran = AtomicUsize::new(0);
            let jobs = vec![job(1, 20, 100), job(2, 10, 1)];
            let error = build_squares(jobs, 100, |square| {
                ran.fetch_add(1, Ordering::Relaxed);
                match square.x {
                    1 => std::panic::panic_any(DropPanics),
                    other => Ok(format!("square {other}/0: ok")),
                }
            })
            .unwrap_err();
            (error, ran.load(Ordering::Relaxed))
        });
        assert!(error.contains("1/0"), "{error}");
        assert_eq!(ran, 1);
    }
}
