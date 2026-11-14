//! Realtime (SCHED_FIFO) scheduling for the audio thread and DSP workers.
//!
//! Without it the audio thread competes with the UI, networking and disk I/O
//! under the normal time-sharing scheduler, and any burst of other work can
//! make it miss a deadline (an xrun). SCHED_FIFO threads preempt all normal
//! threads until they block.
//!
//! Requires `RLIMIT_RTPRIO` >= the requested priority: the shipped systemd
//! unit sets `LimitRTPRIO=95`; on a desktop, membership of a group with an
//! `rtprio` entry in `/etc/security/limits.d` does the same. When the request
//! is refused the thread simply stays at its normal priority, as before.
//!
//! The DSP workers run one step below the audio thread: high enough that
//! normal threads cannot delay a fork, never able to preempt the thread that
//! is waiting for them. If the audio thread is not realtime, the workers are
//! not promoted either, so they can never outrank it.
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

/// Default SCHED_FIFO priority (1-99) for the audio thread: high enough to
/// preempt everything else in user space, below 80+ which is conventionally
/// left for the sound card's (and timers') kernel interrupt threads.
pub const DEFAULT_AUDIO_PRIORITY: i32 = 70;

/// 0 = not realtime (not yet promoted, disabled, or refused).
static AUDIO_PRIORITY: AtomicI32 = AtomicI32::new(0);
/// Lowest priority any DSP worker obtained (0 = none realtime).
static WORKER_PRIORITY: AtomicI32 = AtomicI32::new(0);
/// The refusal explanation is logged once, not on every audio restart.
static REFUSAL_LOGGED: AtomicBool = AtomicBool::new(false);

/// Priority requested for the audio thread: `PEDAL_RT_PRIORITY` (0 disables)
/// or [`DEFAULT_AUDIO_PRIORITY`].
pub fn requested_audio_priority() -> i32 {
    parse_priority(std::env::var("PEDAL_RT_PRIORITY").ok().as_deref())
}

fn parse_priority(value: Option<&str>) -> i32 {
    match value.map(str::trim) {
        Some(text) => text
            .parse::<i32>()
            .map_or(DEFAULT_AUDIO_PRIORITY, |p| p.clamp(0, 99)),
        None => DEFAULT_AUDIO_PRIORITY,
    }
}

/// Priority the audio thread actually runs at (0 = normal scheduling).
pub fn audio_priority() -> i32 {
    AUDIO_PRIORITY.load(Ordering::Relaxed)
}

/// Priority the DSP workers run at (0 = normal scheduling).
pub fn worker_priority() -> i32 {
    WORKER_PRIORITY.load(Ordering::Relaxed)
}

/// Switches the calling thread to SCHED_FIFO at `priority` (1-99).
pub fn set_current_thread_fifo(priority: i32) -> Result<(), String> {
    if !(1..=99).contains(&priority) {
        return Err(format!("realtime priority {priority} is outside 1-99"));
    }
    #[cfg(target_os = "linux")]
    {
        let parameters = libc::sched_param {
            sched_priority: priority,
        };
        // SAFETY: pthread_self() is always valid for the calling thread and
        // `parameters` is a fully initialized sched_param.
        let status = unsafe {
            libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &parameters)
        };
        if status == 0 {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(status).to_string())
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("realtime scheduling is only implemented on Linux".to_owned())
    }
}

/// Returns the calling thread to normal (SCHED_OTHER) scheduling.
pub fn set_current_thread_normal() {
    #[cfg(target_os = "linux")]
    {
        let parameters = libc::sched_param { sched_priority: 0 };
        // SAFETY: as in `set_current_thread_fifo`. Lowering priority needs
        // no privilege, so this cannot fail for the calling thread.
        unsafe {
            libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_OTHER, &parameters)
        };
    }
}

/// The calling thread's SCHED_FIFO priority, or `None` if it is not FIFO.
pub fn current_thread_fifo_priority() -> Option<i32> {
    #[cfg(target_os = "linux")]
    {
        let mut policy = 0;
        let mut parameters = libc::sched_param { sched_priority: 0 };
        // SAFETY: valid thread handle and out-pointers for the call's duration.
        let status = unsafe {
            libc::pthread_getschedparam(libc::pthread_self(), &mut policy, &mut parameters)
        };
        (status == 0 && policy == libc::SCHED_FIFO).then_some(parameters.sched_priority)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Promotes the calling (audio) thread. Call at the start of the audio loop;
/// not realtime-safe itself (one syscall, plus a log line on change).
pub fn promote_audio_thread() -> i32 {
    let requested = requested_audio_priority();
    let obtained = if requested == 0 {
        0
    } else {
        match set_current_thread_fifo(requested) {
            Ok(()) => requested,
            Err(error) => {
                if !REFUSAL_LOGGED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "realtime priority {requested} refused ({error}); audio thread stays at \
                         normal priority. Grant rtprio (systemd LimitRTPRIO, or an rtprio entry \
                         in /etc/security/limits.d) or set PEDAL_RT_PRIORITY=0 to silence this."
                    );
                }
                0
            }
        }
    };
    let previous = AUDIO_PRIORITY.swap(obtained, Ordering::Relaxed);
    if obtained > 0 && previous != obtained {
        eprintln!("audio thread running SCHED_FIFO priority {obtained}");
    }
    obtained
}

/// Priority a DSP worker should run at for the current audio priority.
pub fn desired_worker_priority() -> i32 {
    (audio_priority() - 1).max(0)
}

/// Moves the calling DSP worker to [`desired_worker_priority`] (or back to
/// normal scheduling when the audio thread is not realtime). Workers call
/// this whenever the audio priority changes, since the pool can start
/// before the audio thread promotes itself. Returns the priority obtained.
pub fn promote_worker_thread() -> i32 {
    let priority = desired_worker_priority();
    if priority == 0 {
        if current_thread_fifo_priority().is_some() {
            set_current_thread_normal();
        }
        return 0;
    }
    match set_current_thread_fifo(priority) {
        Ok(()) => {
            // Record the lowest priority any worker holds.
            let _ = WORKER_PRIORITY.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(if current == 0 {
                    priority
                } else {
                    current.min(priority)
                })
            });
            priority
        }
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_setting_parses_and_clamps() {
        assert_eq!(parse_priority(None), DEFAULT_AUDIO_PRIORITY);
        assert_eq!(parse_priority(Some("0")), 0);
        assert_eq!(parse_priority(Some(" 85 ")), 85);
        assert_eq!(parse_priority(Some("120")), 99);
        assert_eq!(parse_priority(Some("-4")), 0);
        assert_eq!(parse_priority(Some("high")), DEFAULT_AUDIO_PRIORITY);
    }

    #[test]
    fn invalid_priorities_are_rejected_without_a_syscall() {
        assert!(set_current_thread_fifo(0).is_err());
        assert!(set_current_thread_fifo(100).is_err());
    }

    /// Succeeds where rtprio is granted and must fail cleanly (leaving the
    /// thread at normal priority) where it is not, e.g. on a dev desktop.
    #[cfg(target_os = "linux")]
    #[test]
    fn promotion_either_takes_effect_or_leaves_the_thread_unchanged() {
        std::thread::spawn(|| match set_current_thread_fifo(10) {
            Ok(()) => assert_eq!(current_thread_fifo_priority(), Some(10)),
            Err(_) => assert_eq!(current_thread_fifo_priority(), None),
        })
        .join()
        .unwrap();
    }

    #[test]
    fn workers_never_promote_when_the_audio_thread_is_not_realtime() {
        // AUDIO_PRIORITY is 0 unless an audio loop promoted itself; this test
        // binary never runs one.
        std::thread::spawn(|| {
            if audio_priority() == 0 {
                assert_eq!(promote_worker_thread(), 0);
                assert_eq!(current_thread_fifo_priority(), None);
            }
        })
        .join()
        .unwrap();
    }
}
