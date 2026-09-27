//! The threads that run the turns of a parallel loop (§19.2), for both modes.
//!
//! The turns are split into chunks of consecutive turns, which the threads take in
//! their order. Each chunk gives its result, and the results are read in the order of
//! the chunks: the program sees what the turns one after the other would give. The first
//! chunk in which a turn leaves the loop (a bug, `break`, `return`) decides what happens
//! next; the chunks after it do not count, and those not started yet do not start.

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::shared;
use crate::value::Value;

/// The number of turns of a loop over `sequence`: a Range, a List, a Set or a Map.
pub fn turn_count(sequence: &Value) -> usize {
    match sequence {
        Value::Range(bounds) if bounds[0] > bounds[1] => 0,
        Value::Range(bounds) => {
            (i128::from(bounds[1]) - i128::from(bounds[0]) + 1).min(usize::MAX as i128) as usize
        }
        other => shared::sequence_len(other),
    }
}

/// The value of the variable of the loop at the turn `turn`, from 0.
pub fn turn_value(sequence: &Value, turn: usize) -> Value {
    match sequence {
        Value::Range(bounds) => Value::Int(bounds[0] + turn as i64),
        other => shared::sequence_at(other, turn),
    }
}

/// The number of threads that run the turns of a parallel loop: the number of cores,
/// or `LION_THREADS` when it is set to a positive number.
pub fn threads() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| {
        std::env::var("LION_THREADS")
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .filter(|&count: &usize| count >= 1)
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |count| count.get()))
    })
}

/// The tasks that run on threads of their own now.
static RUNNING_TASKS: AtomicUsize = AtomicUsize::new(0);

/// A task, as it is given to a thread.
pub type BoxedTask<T> = Box<dyn FnOnce() -> T + Send>;

/// Starts `task` on a thread of its own, with a stack of one of `stack_sizes`, unless
/// `threads()` tasks run already or the system gives no thread: then the task comes
/// back, to run now (C85).
pub fn spawn_task<T: Send + 'static>(
    stack_sizes: &[usize],
    task: impl FnOnce() -> T + Send + 'static,
) -> Result<std::thread::JoinHandle<T>, BoxedTask<T>> {
    let task: BoxedTask<T> = Box::new(task);
    let running = RUNNING_TASKS.fetch_add(1, Ordering::AcqRel);
    if running >= threads() {
        RUNNING_TASKS.fetch_sub(1, Ordering::AcqRel);
        return Err(task);
    }
    // The task is shared with the attempts, and taken by the thread that starts.
    let slot = std::sync::Arc::new(Mutex::new(Some(task)));
    for &size in stack_sizes {
        let taken = std::sync::Arc::clone(&slot);
        let started = std::thread::Builder::new().stack_size(size).spawn(move || {
            let task =
                taken.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the task is there");
            let result = task();
            RUNNING_TASKS.fetch_sub(1, Ordering::AcqRel);
            result
        });
        if let Ok(thread) = started {
            return Ok(thread);
        }
    }
    RUNNING_TASKS.fetch_sub(1, Ordering::AcqRel);
    let task = slot.lock().unwrap_or_else(PoisonError::into_inner).take().expect("no thread took the task");
    Err(task)
}

/// How many chunks each thread gets on average: more chunks share the work better when
/// the turns do not all take the same time.
const CHUNKS_PER_THREAD: usize = 8;

/// Runs the turns `0..turns` by chunks on `threads` threads, each with a stack of one of
/// the `stack_sizes` (the first that the system gives). Gives the result of each chunk,
/// in their order, up to the first one that `stops`, included.
pub fn run_chunks<R: Send>(
    turns: usize,
    threads: usize,
    stack_sizes: &[usize],
    work: &(dyn Fn(Range<usize>) -> R + Sync),
    stops: &(dyn Fn(&R) -> bool + Sync),
) -> Vec<R> {
    let count = turns.clamp(1, threads.max(1) * CHUNKS_PER_THREAD);
    let bound = |chunk: usize| (turns as u128 * chunk as u128 / count as u128) as usize;
    let next = AtomicUsize::new(0);
    let first_stop = AtomicUsize::new(usize::MAX);
    let results: Vec<Mutex<Option<R>>> = (0..count).map(|_| Mutex::new(None)).collect();
    let worker = &|| {
        loop {
            let chunk = next.fetch_add(1, Ordering::Relaxed);
            if chunk >= count || chunk > first_stop.load(Ordering::Relaxed) {
                break;
            }
            let result = work(bound(chunk)..bound(chunk + 1));
            if stops(&result) {
                first_stop.fetch_min(chunk, Ordering::Relaxed);
            }
            *results[chunk].lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
        }
    };
    std::thread::scope(|scope| {
        let mut started = 0;
        for _ in 0..threads.min(count) {
            let spawned = stack_sizes.iter().any(|&size| {
                std::thread::Builder::new().stack_size(size).spawn_scoped(scope, worker).is_ok()
            });
            started += usize::from(spawned);
        }
        // Without a thread, the turns still run, on this one.
        if started == 0 {
            worker();
        }
    });
    let mut ordered = Vec::new();
    for slot in results {
        let Some(result) = slot.into_inner().unwrap_or_else(PoisonError::into_inner) else { break };
        let stop = stops(&result);
        ordered.push(result);
        if stop {
            break;
        }
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chunks_cover_the_turns_in_order() {
        let chunks = run_chunks(100, 4, &[1 << 20], &|range| range, &|_| false);
        assert_eq!(chunks.first().map(|range| range.start), Some(0));
        assert_eq!(chunks.last().map(|range| range.end), Some(100));
        assert!(chunks.windows(2).all(|pair| pair[0].end == pair[1].start));
    }

    #[test]
    fn the_first_chunk_that_stops_is_the_last_result() {
        let chunks = run_chunks(1000, 3, &[1 << 20], &|range| range, &|range| range.contains(&500));
        let last = chunks.last().expect("a chunk");
        assert!(last.contains(&500));
        assert_eq!(chunks.first().map(|range| range.start), Some(0));
    }

    #[test]
    fn the_chunks_run_on_several_threads() {
        let work = |_range: Range<usize>| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            std::thread::current().id()
        };
        let threads = run_chunks(8, 4, &[1 << 20], &work, &|_| false);
        let distinct: std::collections::HashSet<_> = threads.into_iter().collect();
        assert!(distinct.len() > 1, "the chunks ran on one thread");
    }

    #[test]
    fn no_turn_gives_one_empty_chunk() {
        let chunks = run_chunks(0, 4, &[1 << 20], &|range| range.len(), &|_| false);
        assert_eq!(chunks, vec![0]);
    }
}
