// SPDX-License-Identifier: Apache-2.0
//! Taking turns at a board.
//!
//! Two invocations that talk to the same board at once must not interleave.
//! On Linux both would read every answer, so one could take the other's
//! answer for its own. The second one therefore waits until the first is
//! done, up to a limit, and then gives up with a message.
//!
//! How the turn is held differs by system:
//!
//! - **Linux** lets any number of programs open the hidraw node. Here the node
//!   is opened a second time, for nothing but an advisory `flock(LOCK_EX)`,
//!   while the handle the exchange runs over stays as it is. The kernel drops
//!   the lock when the process ends, however it ends, so there is no lock
//!   file to leave behind or clean up.
//! - **macOS** has no lock here: a refused open is repeated until it goes
//!   through. That serializes nothing today. The refusal was observed with
//!   the earlier hidapi transport, which asked for exclusive access; the
//!   current transport opens without that option, and on 2026-10-11 a second
//!   open of a held board went through on a Mac and both handles received
//!   every answer. So today the retry fires only for an open refused for
//!   another reason, such as a board being unplugged or a missing
//!   permission, and then costs the wait before the error is reported. It
//!   is kept for the planned open in seize mode in the transport, which is
//!   expected to make macOS refuse a second open (not yet measured).
//!
//! The lock is advisory. It orders the programs that ask for it - every
//! instance of `ykushctrl`, and any program built on this library - among
//! themselves. A tool that opens the node without asking is not held back.

use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// How long an invocation waits for a board that another one is using.
///
/// One invocation holds a board for a few milliseconds: it finds the board,
/// opens it, sends one report and reads one answer. Five seconds lets dozens
/// of invocations queue up behind each other without any of them failing. A
/// board that stays taken for longer is held by something that does not let
/// go after one command - a program that keeps a handle open, or one stuck in
/// its own five second read timeout - and waiting longer would not help; the
/// caller learns that within the same time the read timeout already allows.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(5);

/// Pause between two attempts. Short against the time one invocation holds
/// the board, long enough that a waiting process does not spin.
const POLL: Duration = Duration::from_millis(10);

/// The outcome of one attempt at getting hold of the board.
pub(crate) enum Attempt<T> {
    /// The board is ours until `T` is dropped.
    Got(T),
    /// Someone else has the board; a later attempt may succeed. Carries what
    /// to tell the user if waiting runs out.
    Busy(String),
    /// Failed for a reason that waiting does not cure.
    Failed(Error),
}

/// Repeats `attempt` until it succeeds, fails for good, or `wait` has passed.
///
/// With `wait` zero, exactly one attempt is made. `serial` names the board in
/// the message when waiting runs out.
pub(crate) fn wait_for<T>(
    serial: &str,
    wait: Duration,
    mut attempt: impl FnMut() -> Attempt<T>,
) -> Result<T> {
    let start = Instant::now();

    loop {
        match attempt() {
            Attempt::Got(held) => return Ok(held),
            Attempt::Failed(error) => return Err(error),
            Attempt::Busy(cause) => {
                let waited = start.elapsed();
                if waited >= wait {
                    return Err(Error::Busy {
                        serial: serial.to_owned(),
                        waited: wait,
                        cause,
                    });
                }
                std::thread::sleep(POLL.min(wait - waited));
            }
        }
    }
}

/// How often the board may change its device node while a turn is being
/// taken before giving up. Once is a replug; more is a board that keeps
/// re-enumerating, which no command will get through to.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const NODE_ROUNDS: usize = 3;

/// Takes the turn at a board and makes sure it is still the board's turn.
///
/// The node is found first and locked second. If the board re-enumerated in
/// between - a replug, a reset by another invocation - the lock sits on a
/// node that belongs to nothing or to another device. So after locking, the
/// node is looked up again; if it changed, the lock is let go and the board
/// is chosen afresh. `wait` bounds the whole of it, and a board in use is
/// reported with the full `wait`, not with what was left in the last round.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn take_stable<D, L, N: PartialEq + std::fmt::Debug>(
    wait: Duration,
    mut choose: impl FnMut() -> Result<(String, D)>,
    node_of: impl Fn(&D) -> Result<N>,
    take: impl Fn(&N, &str, Duration) -> Result<L>,
) -> Result<(String, D, L)> {
    // A wait too long to add to the clock, Duration::MAX for one, has no
    // deadline: it waits as long as it takes.
    let deadline = Instant::now().checked_add(wait);

    for _ in 0..NODE_ROUNDS {
        let (found, device) = choose()?;
        let node = node_of(&device)?;
        let left = deadline.map_or(wait, |d| d.saturating_duration_since(Instant::now()));

        let held = match take(&node, &found, left) {
            Ok(held) => held,
            Err(Error::Busy { serial, cause, .. }) => {
                return Err(Error::Busy {
                    serial,
                    waited: wait,
                    cause,
                })
            }
            Err(error) => return Err(error),
        };

        if node_of(&device).ok().as_ref() == Some(&node) {
            return Ok((found, device, held));
        }
        drop(held);
    }

    Err(Error::Device(
        "The YKUSH3 kept changing its device node while waiting for it; check the cable and \
         try again"
            .into(),
    ))
}

#[cfg(target_os = "linux")]
pub(crate) use self::linux::NodeLock;

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::File;
    use std::path::Path;
    use std::time::Duration;

    use rustix::fs::{flock, FlockOperation};
    use rustix::io::Errno;

    use super::{wait_for, Attempt};
    use crate::error::{Error, Result};

    /// What the user is told when another instance holds the board.
    pub(crate) const HELD: &str = "another ykushctrl is using it";

    /// An exclusive `flock` on the hidraw node of a board, held until dropped.
    ///
    /// The file is opened only to carry the lock; nothing is ever read from it
    /// or written to it. Closing it - on drop, or by the kernel when the
    /// process ends - releases the lock.
    pub(crate) struct NodeLock {
        _node: File,
    }

    impl NodeLock {
        /// Takes the lock on `node`, waiting up to `wait` for another holder
        /// to let go. `serial` names the board in the message.
        pub(crate) fn take(node: &Path, serial: &str, wait: Duration) -> Result<NodeLock> {
            let file = File::open(node).map_err(|error| {
                if error.kind() == std::io::ErrorKind::PermissionDenied {
                    Error::Hid(crate::device::PERMISSION_DENIED.to_owned())
                } else {
                    Error::Hid(format!("Cannot open {}: {error}", node.display()))
                }
            })?;

            wait_for(serial, wait, || {
                match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                    Ok(()) => Attempt::Got(()),
                    Err(Errno::WOULDBLOCK) => Attempt::Busy(HELD.to_owned()),
                    // Interrupted by a signal: not taken, but nothing wrong.
                    Err(Errno::INTR) => {
                        Attempt::Busy("taking the lock was interrupted by a signal".to_owned())
                    }
                    Err(error) => Attempt::Failed(Error::Hid(format!(
                        "Cannot lock {}: {error}",
                        node.display()
                    ))),
                }
            })?;

            Ok(NodeLock { _node: file })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    use crate::device::{Report, Transport};
    use crate::fake::FakeBoard;
    use crate::ykush3::{Port, Ykush3};

    // The tests count attempts rather than time where they can, and where
    // they measure time they only check the lower bound the code guarantees,
    // or a generous upper one. A loaded machine must not fail them.

    // -- a lock that lives in this process --------------------------------
    //
    // Stands in for the board being taken, on every system, so the waiting
    // itself can be tested without a hidraw node.

    #[derive(Clone, Default)]
    struct FakeLock(Arc<AtomicBool>);

    struct FakeGuard(Arc<AtomicBool>);

    impl Drop for FakeGuard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    impl FakeLock {
        fn attempt(&self) -> Attempt<FakeGuard> {
            if self.0.swap(true, Ordering::SeqCst) {
                Attempt::Busy("held by the test".into())
            } else {
                Attempt::Got(FakeGuard(self.0.clone()))
            }
        }

        fn take(&self, wait: Duration) -> Result<FakeGuard> {
            wait_for("Y3TEST", wait, || self.attempt())
        }

        fn is_held(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// Something holding the board on another thread until released.
    struct Holder {
        release: mpsc::Sender<()>,
        thread: thread::JoinHandle<()>,
    }

    impl Holder {
        /// Takes the board with `take` on another thread and returns once it
        /// is held.
        fn hold<G: 'static>(take: impl FnOnce() -> Result<G> + Send + 'static) -> Holder {
            let taken = Arc::new(Barrier::new(2));
            let signal = taken.clone();
            let (release, released) = mpsc::channel();
            let thread = thread::spawn(move || {
                let guard = take().expect("the holder should get the board");
                signal.wait();
                let _ = released.recv();
                drop(guard);
            });
            taken.wait();
            Holder { release, thread }
        }

        fn release(self) {
            self.release.send(()).unwrap();
            self.thread.join().unwrap();
        }
    }

    /// A transport that holds the turn for as long as it lives, the way
    /// `Board` does.
    struct Turned<G> {
        board: FakeBoard,
        _turn: G,
    }

    impl<G> Transport for Turned<G> {
        fn transfer(&self, out: &Report) -> Result<Report> {
            self.board.transfer(out)
        }

        fn send(&self, out: &Report) -> Result<()> {
            self.board.send(out)
        }
    }

    // -- scenarios, run against the fake lock and against flock -----------

    /// Two invocations started together: each takes its turn, reads a port
    /// state, and lets go. What is checked is that the time spans in which
    /// they hold the board never overlap.
    fn two_invocations_take_turns<G: 'static>(
        take: impl Fn() -> Result<G> + Send + Sync + 'static,
    ) {
        let take = Arc::new(take);
        let spans = Arc::new(Mutex::new(Vec::new()));
        let start = Arc::new(Barrier::new(2));

        let invocations: Vec<_> = (0..2)
            .map(|_| {
                let (take, spans, start) = (take.clone(), spans.clone(), start.clone());
                thread::spawn(move || {
                    start.wait();
                    let board = Ykush3::with_transport(Turned {
                        board: FakeBoard::answering(&[0x01, 0x11]),
                        _turn: take().expect("both invocations should get the board"),
                    });
                    let from = Instant::now();
                    // Long enough that an overlap could not hide.
                    thread::sleep(Duration::from_millis(100));
                    let status = board.port_status(Port::Downstream(1)).unwrap();
                    let to = Instant::now();
                    drop(board);
                    spans.lock().unwrap().push((from, to));
                    assert!(status.on);
                })
            })
            .collect();

        for invocation in invocations {
            invocation.join().expect("an invocation failed");
        }

        let mut spans = spans.lock().unwrap().clone();
        spans.sort();
        assert_eq!(spans.len(), 2);
        assert!(
            spans[0].1 <= spans[1].0,
            "the two invocations held the board at the same time: {spans:?}"
        );
    }

    /// A command that fails still lets go of the board, so the next one gets
    /// it without waiting.
    fn a_failed_command_lets_go_of_the_board<G>(take: impl Fn(Duration) -> Result<G>) {
        let invocation = || -> Result<()> {
            let board = Ykush3::with_transport(Turned {
                board: FakeBoard::failing(Error::NoResponse),
                _turn: take(DEFAULT_WAIT)?,
            });
            board.port_status(Port::Downstream(1))?;
            Ok(())
        };

        assert!(matches!(invocation(), Err(Error::NoResponse)));
        take(Duration::ZERO).expect("the board should be free after the failure");
    }

    /// While the board is held, waiting runs out at the limit and not before.
    fn waiting_runs_out_at_the_limit<G: 'static>(
        take: impl Fn(Duration) -> Result<G> + Send + Sync + 'static,
    ) -> Error {
        let take = Arc::new(take);
        let holder = {
            let take = take.clone();
            Holder::hold(move || take(Duration::ZERO))
        };

        let started = Instant::now();
        let result = take(Duration::from_millis(200));
        let took = started.elapsed();
        holder.release();

        assert!(
            took >= Duration::from_millis(200),
            "gave up early: {took:?}"
        );
        assert!(took < Duration::from_secs(4), "gave up late: {took:?}");
        match result {
            Err(error @ Error::Busy { .. }) => error,
            Err(other) => panic!("expected Busy, got {other}"),
            Ok(_) => panic!("expected Busy, got the board"),
        }
    }

    // -- the fake lock ----------------------------------------------------

    #[test]
    fn a_free_board_is_taken_with_one_attempt() {
        let attempts = AtomicUsize::new(0);
        let lock = FakeLock::default();

        let guard = wait_for("Y3TEST", DEFAULT_WAIT, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            lock.attempt()
        });

        assert!(guard.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn two_invocations_run_one_after_the_other() {
        let lock = FakeLock::default();

        two_invocations_take_turns(move || lock.take(DEFAULT_WAIT));
    }

    #[test]
    fn a_waiting_invocation_gets_the_board_once_it_is_free() {
        let lock = FakeLock::default();
        let holder = {
            let lock = lock.clone();
            Holder::hold(move || lock.take(Duration::ZERO))
        };
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            holder.release();
        });

        lock.take(DEFAULT_WAIT)
            .expect("the board should come free in time");
        releaser.join().unwrap();
    }

    #[test]
    fn waiting_gives_up_after_the_limit() {
        let lock = FakeLock::default();

        let error = waiting_runs_out_at_the_limit(move |wait| lock.take(wait));

        match error {
            Error::Busy {
                serial,
                waited,
                cause,
            } => {
                assert_eq!(serial, "Y3TEST");
                assert_eq!(waited, Duration::from_millis(200));
                assert_eq!(cause, "held by the test");
            }
            other => panic!("expected Busy, got {other}"),
        }
    }

    #[test]
    fn without_waiting_a_taken_board_fails_after_one_attempt() {
        let lock = FakeLock::default();
        let _held = lock.take(Duration::ZERO).unwrap();
        let attempts = AtomicUsize::new(0);

        let result = wait_for("Y3TEST", Duration::ZERO, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            lock.attempt()
        });

        assert!(matches!(result, Err(Error::Busy { waited, .. }) if waited.is_zero()));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failure_waiting_cannot_cure_is_reported_at_once() {
        let attempts = AtomicUsize::new(0);

        let result: Result<()> = wait_for("Y3TEST", DEFAULT_WAIT, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            Attempt::Failed(Error::NotFound { serial: None })
        });

        assert!(matches!(result, Err(Error::NotFound { .. })));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_command_releases_the_board() {
        let lock = FakeLock::default();

        a_failed_command_lets_go_of_the_board(|wait| lock.take(wait));
    }

    // -- a node that changes while the turn is taken ----------------------

    #[test]
    fn a_stable_node_is_taken_in_one_round() {
        let lock = FakeLock::default();
        let chosen = AtomicUsize::new(0);

        let (found, node, _held) = take_stable(
            DEFAULT_WAIT,
            || {
                chosen.fetch_add(1, Ordering::SeqCst);
                Ok(("Y3TEST".to_owned(), "hidraw0"))
            },
            |device| Ok(*device),
            |_, _, wait| lock.take(wait),
        )
        .unwrap();

        assert_eq!((found.as_str(), node), ("Y3TEST", "hidraw0"));
        assert_eq!(chosen.load(Ordering::SeqCst), 1);
        assert!(lock.is_held());
    }

    #[test]
    fn a_node_that_moved_while_locking_is_let_go_and_chosen_again() {
        let lock = FakeLock::default();
        let chosen = AtomicUsize::new(0);
        let lookups = AtomicUsize::new(0);

        // The first board found re-enumerates right after it was locked: its
        // second lookup names another node. The next round finds it settled.
        let (_, node, _held) = take_stable(
            DEFAULT_WAIT,
            || {
                let round = chosen.fetch_add(1, Ordering::SeqCst);
                Ok(("Y3TEST".to_owned(), round))
            },
            |round| {
                let lookup = lookups.fetch_add(1, Ordering::SeqCst);
                Ok(if *round == 0 && lookup == 1 {
                    "hidraw1"
                } else if *round == 0 {
                    "hidraw0"
                } else {
                    "hidraw1"
                })
            },
            |node, _, wait| {
                // The lock of the first round must be gone before the second.
                assert!(!lock.is_held(), "{node} locked while still holding");
                lock.take(wait)
            },
        )
        .unwrap();

        assert_eq!(node, 1);
        assert_eq!(chosen.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_node_that_keeps_moving_is_given_up_on() {
        let lock = FakeLock::default();
        let lookups = AtomicUsize::new(0);

        let result = take_stable(
            DEFAULT_WAIT,
            || Ok(("Y3TEST".to_owned(), ())),
            |_| Ok(lookups.fetch_add(1, Ordering::SeqCst)),
            |_, _, wait| lock.take(wait),
        );

        assert!(matches!(&result, Err(Error::Device(msg)) if msg.contains("kept changing")));
        assert!(!lock.is_held(), "the last lock should be let go");
    }

    #[test]
    fn an_endless_wait_takes_a_free_board() {
        let lock = FakeLock::default();

        let (_, _, _held) = take_stable(
            Duration::MAX,
            || Ok(("Y3TEST".to_owned(), ())),
            |_| Ok(()),
            |_, _, wait| lock.take(wait),
        )
        .expect("a free board should be taken however long the wait");

        assert!(lock.is_held());
    }

    #[test]
    fn a_board_in_use_is_reported_with_the_whole_wait() {
        let lock = FakeLock::default();
        let _held = lock.take(Duration::ZERO).unwrap();

        let result = take_stable(
            Duration::from_millis(50),
            || Ok(("Y3TEST".to_owned(), ())),
            |_| Ok(()),
            |_, _, wait| lock.take(wait),
        );

        assert!(matches!(
            result,
            Err(Error::Busy { waited, .. }) if waited == Duration::from_millis(50)
        ));
    }

    // -- the real flock, on a file standing in for the hidraw node -------

    #[cfg(target_os = "linux")]
    mod flock {
        use super::*;
        use crate::lock::NodeLock;
        use std::path::PathBuf;

        /// A file in the temporary directory, removed again when dropped.
        struct Node(PathBuf);

        impl Node {
            fn new(name: &str) -> Self {
                let path =
                    std::env::temp_dir().join(format!("ykush3-node-{}-{name}", std::process::id()));
                std::fs::write(&path, b"").unwrap();
                Node(path)
            }

            fn take(&self, wait: Duration) -> Result<NodeLock> {
                NodeLock::take(&self.0, "Y3TEST", wait)
            }
        }

        impl Drop for Node {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }

        #[test]
        fn two_invocations_run_one_after_the_other_under_flock() {
            let node = Arc::new(Node::new("turns"));

            two_invocations_take_turns(move || node.take(DEFAULT_WAIT));
        }

        #[test]
        fn flock_gives_up_after_the_limit() {
            let node = Arc::new(Node::new("limit"));

            let error = waiting_runs_out_at_the_limit(move |wait| node.take(wait));

            assert!(
                matches!(&error, Error::Busy { cause, .. } if cause == crate::lock::linux::HELD),
                "{error}"
            );
        }

        #[test]
        fn without_waiting_flock_fails_at_once_and_succeeds_once_free() {
            let node = Node::new("nowait");
            let held = node.take(Duration::ZERO).expect("a free node");

            assert!(matches!(
                node.take(Duration::ZERO),
                Err(Error::Busy { waited, .. }) if waited.is_zero()
            ));

            drop(held);
            node.take(Duration::ZERO)
                .expect("the node should be free once the holder let go");
        }

        #[test]
        fn a_failed_command_releases_the_flock() {
            let node = Node::new("error");

            a_failed_command_lets_go_of_the_board(|wait| node.take(wait));
        }

        #[test]
        fn a_missing_node_is_an_error_not_a_wait() {
            let started = Instant::now();
            let result = NodeLock::take(
                std::path::Path::new("/dev/ykush3-no-such-node"),
                "Y3TEST",
                DEFAULT_WAIT,
            );

            assert!(matches!(&result, Err(Error::Hid(msg)) if msg.contains("Cannot open")));
            // Generous: the point is that it did not wait the five seconds.
            assert!(started.elapsed() < DEFAULT_WAIT / 2);
        }
    }
}
