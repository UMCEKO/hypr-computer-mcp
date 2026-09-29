//! Keeps the session from idling or sleeping while the tool is in use, and lets it go after a quiet spell.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use zbus::blocking::Connection;
use zbus::zvariant::OwnedFd;

const WHO: &str = "hypr-computer";
const WHY: &str = "An agent is driving the desktop";

/// What holds the session awake; dropping the fd ends logind's inhibitor.
struct Held {
    screensaver: Option<(Connection, u32)>,
    _logind: Option<OwnedFd>,
}

impl Held {
    fn take() -> Self {
        let screensaver = Connection::session()
            .and_then(|bus| {
                let cookie = bus
                    .call_method(
                        Some("org.freedesktop.ScreenSaver"),
                        "/org/freedesktop/ScreenSaver",
                        Some("org.freedesktop.ScreenSaver"),
                        "Inhibit",
                        &(WHO, WHY),
                    )?
                    .body()
                    .deserialize::<u32>()?;
                Ok((bus, cookie))
            })
            .inspect_err(|e| eprintln!("{WHO}: no screensaver inhibit: {e}"))
            .ok();
        let logind = Connection::system()
            .and_then(|bus| {
                bus.call_method(
                    Some("org.freedesktop.login1"),
                    "/org/freedesktop/login1",
                    Some("org.freedesktop.login1.Manager"),
                    "Inhibit",
                    &("idle:sleep", WHO, WHY, "block"),
                )?
                .body()
                .deserialize::<OwnedFd>()
            })
            .inspect_err(|e| eprintln!("{WHO}: no logind inhibit: {e}"))
            .ok();
        Self {
            screensaver,
            _logind: logind,
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some((bus, cookie)) = self.screensaver.take() {
            let _ = bus.call_method(
                Some("org.freedesktop.ScreenSaver"),
                "/org/freedesktop/ScreenSaver",
                Some("org.freedesktop.ScreenSaver"),
                "UnInhibit",
                &(cookie,),
            );
        }
    }
}

#[derive(Default)]
struct Lease {
    until: Option<Instant>,
    held: Option<Held>,
    closed: bool,
}

pub struct Awake {
    lease: Arc<(Mutex<Lease>, Condvar)>,
    quiet: Duration,
}

fn lock(lease: &Mutex<Lease>) -> MutexGuard<'_, Lease> {
    lease
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Awake {
    /// Inhibitors taken on first use and released once `quiet` passes without another.
    pub fn new(quiet: Duration) -> Self {
        let lease = Arc::new((Mutex::new(Lease::default()), Condvar::new()));
        let watch = Arc::clone(&lease);
        thread::spawn(move || {
            let (mutex, wake) = &*watch;
            let mut lease = lock(mutex);
            while !lease.closed {
                lease = match lease.until {
                    None => wake.wait(lease).unwrap_or_else(|p| p.into_inner()),
                    Some(until) if Instant::now() >= until => {
                        lease.until = None;
                        lease.held = None;
                        lease
                    }
                    Some(until) => {
                        let left = until.saturating_duration_since(Instant::now());
                        wake.wait_timeout(lease, left)
                            .unwrap_or_else(|p| p.into_inner())
                            .0
                    }
                };
            }
        });
        Self { lease, quiet }
    }

    /// Hold the session awake for another quiet spell from now.
    pub fn renew(&self) {
        let (mutex, wake) = &*self.lease;
        let mut lease = lock(mutex);
        if lease.held.is_none() {
            lease.held = Some(Held::take());
        }
        lease.until = Some(Instant::now() + self.quiet);
        wake.notify_one();
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        let (mutex, wake) = &*self.lease;
        let mut lease = lock(mutex);
        lease.closed = true;
        lease.held = None;
        wake.notify_one();
    }
}
