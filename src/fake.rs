// SPDX-License-Identifier: Apache-2.0
//! Test double for the HID transport.
//!
//! Records what a command sends to the board and answers from a prepared queue,
//! so the protocol layer - and any code built on this library - can be
//! exercised without hardware. The inspection methods panic on misuse, which is
//! the right behaviour for a test helper: the panic message is the diagnosis.
//!
//! Sits behind the `fake` feature, precisely because of those panics: a build
//! that does not ask for the test double gets a library without one.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use crate::device::{report, Report, Transport};
use crate::error::{Error, Result};

pub struct FakeBoard {
    sent: RefCell<Vec<Report>>,
    answers: RefCell<VecDeque<Result<Report>>>,
    desyncs: Cell<usize>,
}

impl FakeBoard {
    /// A board that is not expected to be read from. Reading anyway fails the
    /// test, which is how the write-only commands are pinned down.
    pub fn mute() -> Self {
        FakeBoard {
            sent: RefCell::new(Vec::new()),
            answers: RefCell::new(VecDeque::new()),
            desyncs: Cell::new(0),
        }
    }

    /// A board that answers the next request with `payload`, zero padded.
    pub fn answering(payload: &[u8]) -> Self {
        let board = FakeBoard::mute();
        board.answers.borrow_mut().push_back(Ok(report(payload)));
        board
    }

    /// A board whose next request fails in the transport.
    pub fn failing(error: Error) -> Self {
        let board = FakeBoard::mute();
        board.answers.borrow_mut().push_back(Err(error));
        board
    }

    /// The reports that were sent, in order.
    pub fn sent(&self) -> Vec<Report> {
        self.sent.borrow().clone()
    }

    /// First `len` bytes of the single report that was sent.
    pub fn sent_payload(&self, len: usize) -> Vec<u8> {
        let sent = self.sent();
        assert_eq!(sent.len(), 1, "expected exactly one report to be sent");
        sent[0][..len].to_vec()
    }

    /// Number of reports that were sent.
    pub fn sent_count(&self) -> usize {
        self.sent.borrow().len()
    }

    /// How often the protocol layer said the queue cannot be trusted.
    pub fn desyncs(&self) -> usize {
        self.desyncs.get()
    }
}

impl Transport for FakeBoard {
    fn transfer(&self, out: &Report) -> Result<Report> {
        self.send(out)?;

        self.answers
            .borrow_mut()
            .pop_front()
            .expect("command read from the board although no answer was prepared")
    }

    fn send(&self, out: &Report) -> Result<()> {
        self.sent.borrow_mut().push(*out);
        Ok(())
    }

    fn desync(&self) {
        self.desyncs.set(self.desyncs.get() + 1);
    }
}
