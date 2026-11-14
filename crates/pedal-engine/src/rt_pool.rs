//! Realtime fork/join worker pool, a port of Tone3000's `RtWorkerPool`.
//!
//! Used to run the independent phase instances of an oversampled NAM in
//! parallel. Output is bit-identical to running them serially: jobs work on
//! disjoint data and only the scheduling changes.
//!
//! Job protocol (lock-free, one atomic state per slot):
//!
//! ```text
//! Free --publisher CAS--> Building --> Armed --worker CAS--> Claimed --> Done
//!                                        |                                |
//!                                        +--joiner CAS--> run inline --> Free <-- joiner recycles
//! ```
//!
//! The Armed -> Claimed compare-exchange is raced between workers and the
//! publishing thread's join loop, and whoever wins runs the job. That race is
//! the safety valve: if every worker is descheduled or busy, the publisher
//! steals its own jobs back and runs them inline, so an audio block degrades
//! to exactly the serial cost instead of stalling. The publisher always runs
//! the first job itself and never parks; it only spins on jobs a worker is
//! actively executing.
//!
//! Wakeups are targeted: parked workers advertise themselves in a bitmask and
//! a fork wakes only as many as it published jobs. A worker sets its parked
//! bit and then re-checks for armed jobs, while a publisher arms jobs and
//! then reads the mask, with a SeqCst fence pairing the two, so either the
//! worker sees the job or the publisher sees the bit. `Thread::unpark` is a
//! token, so a wake that lands just before `park` is never lost; the long
//! park timeout is only a last-resort re-check.
//!
//! Workers run SCHED_FIFO one step below the audio thread when it is
//! realtime, and at normal priority otherwise, so they never outrank the
//! thread joining them (see `realtime`). They flush
//! denormals to zero like the audio thread does, so parallel and serial
//! output stay identical there.
use std::cell::UnsafeCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering, fence};
use std::thread::{self, Thread};
use std::time::Duration;

/// Slot registry capacity, shared by every concurrent fork (e.g. the live
/// rig and an offline render). A fork that finds it full runs the overflow
/// inline: still correct, just less parallel.
pub const MAX_JOBS: usize = 16;
/// Workers are capped so the parked bitmask fits and an 8x fork (7 published
/// jobs) can be fully parallel.
pub const MAX_WORKERS: usize = 7;
const PARK_TIMEOUT: Duration = Duration::from_millis(100);

const FREE: u8 = 0;
const BUILDING: u8 = 1;
const ARMED: u8 = 2;
const CLAIMED: u8 = 3;
const DONE: u8 = 4;

#[derive(Clone, Copy)]
struct Job {
    run: unsafe fn(*const (), *mut ()),
    function: *const (),
    context: *mut (),
}

impl Job {
    const EMPTY: Self = Self {
        run: noop,
        function: std::ptr::null(),
        context: std::ptr::null_mut(),
    };
}

unsafe fn noop(_: *const (), _: *mut ()) {}

/// Calls the type-erased `fn(&mut T)` on the type-erased `&mut T`.
///
/// # Safety
/// `function` must be a `fn(&mut T)` and `context` a valid, exclusively
/// accessible `*mut T`.
unsafe fn trampoline<T>(function: *const (), context: *mut ()) {
    // SAFETY: guaranteed by the caller (`for_each` builds both from a
    // `fn(&mut T)` and an element of a `&mut [T]`).
    let function: fn(&mut T) = unsafe { std::mem::transmute(function) };
    function(unsafe { &mut *context.cast::<T>() });
}

/// One cache line per slot: states are hammered by CAS from every thread.
#[repr(align(64))]
struct Slot {
    state: AtomicU8,
    job: UnsafeCell<Job>,
}

impl Slot {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(FREE),
            job: UnsafeCell::new(Job::EMPTY),
        }
    }
}

pub struct RtWorkerPool {
    slots: [Slot; MAX_JOBS],
    workers: OnceLock<Vec<Thread>>,
    alive: AtomicUsize,
    /// Bit i set: worker i is parked.
    parked: AtomicU32,
    /// Jobs a worker had to recover from a panic in (should stay 0).
    panicked: AtomicBool,
}

// SAFETY: a slot's `job` is written only by the thread that moved it
// Free -> Building and read only by the thread that won Armed -> Claimed;
// the Release/Acquire state transitions order those accesses. The raw
// pointers inside a job are only dereferenced while the publishing
// `for_each` call is still blocked joining it.
unsafe impl Sync for RtWorkerPool {}
unsafe impl Send for RtWorkerPool {}

static GLOBAL: RtWorkerPool = RtWorkerPool::new();

/// The process-wide pool. Call [`ensure_global_started`] from a non-realtime
/// thread before relying on it being parallel; until then it runs inline.
pub fn global() -> &'static RtWorkerPool {
    &GLOBAL
}

/// Starts the global pool once. Not realtime safe (spawns threads). The
/// worker count is one per spare core, capped at [`MAX_WORKERS`], or
/// `PEDAL_DSP_WORKERS` (0 disables parallelism).
pub fn ensure_global_started() {
    GLOBAL.start(default_worker_count());
}

fn default_worker_count() -> usize {
    if let Some(count) = std::env::var("PEDAL_DSP_WORKERS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
    {
        return count.min(MAX_WORKERS);
    }
    thread::available_parallelism()
        .map(|cores| cores.get().saturating_sub(1))
        .unwrap_or(0)
        .min(MAX_WORKERS)
}

impl RtWorkerPool {
    pub const fn new() -> Self {
        Self {
            slots: [const { Slot::new() }; MAX_JOBS],
            workers: OnceLock::new(),
            alive: AtomicUsize::new(0),
            parked: AtomicU32::new(0),
            panicked: AtomicBool::new(false),
        }
    }

    /// Spawns `count` workers (capped at [`MAX_WORKERS`]) the first time it
    /// is called; later calls do nothing. Workers live for the rest of the
    /// process. Not realtime safe.
    pub fn start(&'static self, count: usize) {
        self.workers.get_or_init(|| {
            (0..count.min(MAX_WORKERS))
                .filter_map(|index| {
                    thread::Builder::new()
                        .name(format!("pedal-dsp-{}", index + 1))
                        .spawn(move || self.worker_loop(index))
                        .ok()
                        .map(|handle| handle.thread().clone())
                })
                .collect()
        });
    }

    /// Workers currently running their loop.
    pub fn worker_count(&self) -> usize {
        self.alive.load(Ordering::Acquire)
    }

    /// True if a worker caught a panic from a job (a bug; the job's output
    /// is then incomplete).
    pub fn job_panicked(&self) -> bool {
        self.panicked.load(Ordering::Relaxed)
    }

    /// Realtime safe: no allocation, no locks. Runs `job` on every item,
    /// potentially in parallel, and returns once all have finished. The
    /// calling thread always runs `items[0]` and any job no worker claims,
    /// so completion never depends on worker liveness.
    pub fn for_each<T: Send>(&self, items: &mut [T], job: fn(&mut T)) {
        let count = items.len();
        if count <= 1 || self.alive.load(Ordering::Acquire) == 0 {
            items.iter_mut().for_each(job);
            return;
        }
        let base = items.as_mut_ptr();
        let mut join = Join {
            pool: self,
            published: [usize::MAX; MAX_JOBS],
            count: 0,
        };
        for index in 1..count {
            // SAFETY: `index < count`, and every element is handed to exactly
            // one job, so the `&mut T`s created from these pointers are
            // disjoint.
            let item = unsafe { base.add(index) };
            let published = (join.count < MAX_JOBS)
                .then(|| {
                    self.try_publish(Job {
                        run: trampoline::<T>,
                        function: job as *const (),
                        context: item.cast(),
                    })
                })
                .flatten();
            match published {
                Some(slot) => {
                    join.published[join.count] = slot;
                    join.count += 1;
                }
                None => job(unsafe { &mut *item }),
            }
        }
        if join.count > 0 {
            // Pairs with the worker's SeqCst parked-bit update (module docs).
            fence(Ordering::SeqCst);
            self.wake_parked(join.count);
        }
        // SAFETY: element 0 was not published.
        job(unsafe { &mut *base });
        // `join` drops here and waits for every published job, also when
        // `job` above unwinds, so no worker can outlive the borrowed items.
    }

    fn try_publish(&self, job: Job) -> Option<usize> {
        self.slots.iter().position(|slot| {
            slot.state.load(Ordering::Relaxed) == FREE
                && slot
                    .state
                    .compare_exchange(FREE, BUILDING, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                && {
                    // SAFETY: Building gives this thread exclusive access.
                    unsafe { *slot.job.get() = job };
                    slot.state.store(ARMED, Ordering::Release);
                    true
                }
        })
    }

    /// Claims and runs one armed job from any fork. Returns whether one ran.
    fn try_run_one(&self) -> bool {
        for slot in &self.slots {
            if slot.state.load(Ordering::Relaxed) != ARMED {
                continue;
            }
            if slot
                .state
                .compare_exchange(ARMED, CLAIMED, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                // SAFETY: Claimed gives this thread exclusive access, and
                // the publisher is still joining, keeping the job's data alive.
                let job = unsafe { *slot.job.get() };
                if catch_unwind(AssertUnwindSafe(|| unsafe {
                    (job.run)(job.function, job.context)
                }))
                .is_err()
                {
                    self.panicked.store(true, Ordering::Relaxed);
                }
                slot.state.store(DONE, Ordering::Release);
                return true;
            }
        }
        false
    }

    fn any_armed(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.state.load(Ordering::Relaxed) == ARMED)
    }

    fn wake_parked(&self, mut wakes: usize) {
        let parked = self.parked.load(Ordering::Relaxed);
        let Some(workers) = self.workers.get() else {
            return;
        };
        for (index, worker) in workers.iter().enumerate() {
            if wakes == 0 {
                break;
            }
            if parked & (1 << index) != 0 {
                worker.unpark();
                wakes -= 1;
            }
        }
    }

    fn worker_loop(&self, index: usize) {
        crate::dsp::flush_denormals_to_zero();
        let bit = 1u32 << index;
        // Follow the audio thread's realtime priority (one step below it).
        // The pool may start before the audio thread promotes itself, so
        // re-check after every wake; it is one relaxed atomic load.
        let mut scheduled_for = crate::realtime::desired_worker_priority();
        crate::realtime::promote_worker_thread();
        self.alive.fetch_add(1, Ordering::Release);
        loop {
            let desired = crate::realtime::desired_worker_priority();
            if desired != scheduled_for {
                scheduled_for = desired;
                crate::realtime::promote_worker_thread();
            }
            if self.try_run_one() {
                continue; // forks come in bursts; rescan immediately
            }
            self.parked.fetch_or(bit, Ordering::SeqCst);
            if self.any_armed() {
                self.parked.fetch_and(!bit, Ordering::Relaxed);
                continue;
            }
            thread::park_timeout(PARK_TIMEOUT);
            self.parked.fetch_and(!bit, Ordering::Relaxed);
        }
    }
}

impl Default for RtWorkerPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Waits for (or steals and runs) every published job of one fork.
struct Join<'a> {
    pool: &'a RtWorkerPool,
    published: [usize; MAX_JOBS],
    count: usize,
}

impl Drop for Join<'_> {
    fn drop(&mut self) {
        let mut remaining = self.count;
        while remaining > 0 {
            let mut progressed = false;
            for entry in &mut self.published[..self.count] {
                if *entry == usize::MAX {
                    continue;
                }
                let slot = &self.pool.slots[*entry];
                let mut state = slot.state.load(Ordering::Acquire);
                if state == ARMED {
                    if slot
                        .state
                        .compare_exchange(ARMED, CLAIMED, Ordering::Acquire, Ordering::Relaxed)
                        .is_ok()
                    {
                        // SAFETY: as in `try_run_one`.
                        let job = unsafe { *slot.job.get() };
                        unsafe { (job.run)(job.function, job.context) };
                        slot.state.store(FREE, Ordering::Release);
                        *entry = usize::MAX;
                        remaining -= 1;
                        progressed = true;
                        continue;
                    }
                    state = slot.state.load(Ordering::Acquire);
                }
                if state == DONE {
                    // Only the publisher recycles its slots, so scanning by
                    // remembered index is ABA-safe.
                    slot.state.store(FREE, Ordering::Release);
                    *entry = usize::MAX;
                    remaining -= 1;
                    progressed = true;
                }
            }
            if !progressed {
                std::hint::spin_loop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn started_pool(workers: usize) -> &'static RtWorkerPool {
        let pool: &'static RtWorkerPool = Box::leak(Box::new(RtWorkerPool::new()));
        pool.start(workers);
        while pool.worker_count() < workers {
            thread::yield_now();
        }
        pool
    }

    fn work(item: &mut (u64, u64)) {
        // Enough work that workers genuinely overlap with the publisher.
        let mut x = item.0;
        for _ in 0..20_000 {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
        }
        item.1 = x;
    }

    fn expected(seed: u64) -> u64 {
        let mut item = (seed, 0);
        work(&mut item);
        item.1
    }

    #[test]
    fn every_item_runs_exactly_once_with_and_without_workers() {
        for workers in [0, 1, 3, 7] {
            let pool = started_pool(workers);
            for count in [0, 1, 2, 5, 8, MAX_JOBS, MAX_JOBS + 5] {
                for _ in 0..50 {
                    let mut items: Vec<(u64, u64)> = (0..count as u64).map(|i| (i, 0)).collect();
                    pool.for_each(&mut items, work);
                    for (seed, result) in items {
                        assert_eq!(result, expected(seed), "{workers} workers, {count} items");
                    }
                }
            }
            assert!(!pool.job_panicked());
        }
    }

    #[test]
    fn concurrent_forks_from_several_threads_share_the_pool() {
        let pool = started_pool(4);
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    for round in 0..300u64 {
                        let mut items: Vec<(u64, u64)> =
                            (0..8).map(|i| (t * 1_000_000 + round * 8 + i, 0)).collect();
                        pool.for_each(&mut items, work);
                        for (seed, result) in items {
                            assert_eq!(result, expected(seed));
                        }
                    }
                })
            })
            .collect();
        for handle in threads {
            handle.join().unwrap();
        }
    }

    #[test]
    fn workers_actually_share_the_load() {
        let pool = started_pool(3);
        fn record(item: &mut (thread::ThreadId, u64)) {
            let mut x = item.1;
            for _ in 0..200_000 {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            }
            item.1 = x;
            item.0 = thread::current().id();
        }
        let me = thread::current().id();
        let mut helped = false;
        for _ in 0..200 {
            let mut items = [(me, 1u64); 4];
            pool.for_each(&mut items, record);
            helped |= items.iter().any(|(id, _)| *id != me);
        }
        assert!(helped, "no job was ever run by a worker");
    }

    #[test]
    fn a_panicking_inline_job_still_joins_published_jobs() {
        let pool = started_pool(2);
        fn maybe_panic(item: &mut (u64, u64)) {
            if item.0 == 0 {
                panic!("job 0");
            }
            work(item);
        }
        let mut items: Vec<(u64, u64)> = (0..4).map(|i| (i, 0)).collect();
        let result = catch_unwind(AssertUnwindSafe(|| pool.for_each(&mut items, maybe_panic)));
        assert!(result.is_err());
        // Every published job finished before the unwind left `for_each`.
        for (seed, value) in &items[1..] {
            assert_eq!(*value, expected(*seed));
        }
        // And the slots were recycled for the next fork.
        let mut again: Vec<(u64, u64)> = (10..14).map(|i| (i, 0)).collect();
        pool.for_each(&mut again, work);
        assert!(again.iter().all(|(seed, value)| *value == expected(*seed)));
    }

    /// Only meaningful where rtprio is granted (e.g. under the systemd unit
    /// or `sudo`); elsewhere promotion is refused and nothing is asserted.
    #[cfg(target_os = "linux")]
    #[test]
    fn workers_follow_the_audio_thread_priority() {
        let audio = thread::spawn(|| {
            let audio = crate::realtime::promote_audio_thread();
            if audio == 0 {
                return None;
            }
            let pool = started_pool(2);
            fn record(item: &mut (Option<i32>, bool)) {
                let mut x = 1u64;
                for _ in 0..200_000 {
                    x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                }
                item.1 = x != 0;
                item.0 = crate::realtime::current_thread_fifo_priority();
            }
            let mut seen = Vec::new();
            for _ in 0..200 {
                let mut items = [(None, false); 3];
                pool.for_each(&mut items, record);
                // Item 0 always runs on the (realtime) audio thread itself.
                assert_eq!(items[0].0, Some(audio));
                seen.extend(items[1..].iter().map(|item| item.0));
            }
            Some((audio, seen))
        })
        .join()
        .unwrap();
        if let Some((audio, seen)) = audio {
            // Jobs ran either on a worker one step below the audio thread or
            // were stolen back by the audio thread itself.
            assert!(
                seen.iter()
                    .all(|p| *p == Some(audio - 1) || *p == Some(audio))
            );
            assert!(
                seen.contains(&Some(audio - 1)),
                "no job ran on a realtime worker"
            );
            assert_eq!(crate::realtime::worker_priority(), audio - 1);
        }
    }
}
