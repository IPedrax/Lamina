//! What the assistant's tests stand in for the network with, and the desktop's tests too: the crate's `testing` feature turns
//! this on for them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::io::{self, Cursor, Read};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::net::{Body, Net, Request};

/// A network that answers from a queue, one reply for each request, and remembers what it was sent.
#[derive(Default)]
pub struct Fake {
    replies: Mutex<VecDeque<Result<(u16, Body), String>>>,
    seen: Mutex<Vec<Request>>,
}

impl Fake {
    /// The next request is answered with `status` and `body`, all of it at once.
    pub fn reply(self, status: u16, body: impl Into<String>) -> Fake {
        self.replies.lock().unwrap().push_back(Ok((status, Box::new(Cursor::new(body.into().into_bytes())))));
        self
    }

    /// The next request is answered with `status` and a body that comes as the test sends it: each text sent is the next piece of
    /// it, and dropping the sender ends it. A send that fails means the reply is no longer being read.
    pub fn live(self, status: u16) -> (Fake, Sender<String>) {
        let (tx, rx) = channel();
        self.replies.lock().unwrap().push_back(Ok((status, Box::new(Live { rx, left: Cursor::default() }))));
        (self, tx)
    }

    /// The next request fails as a dropped connection does.
    pub fn fail(self, error: &str) -> Fake {
        self.replies.lock().unwrap().push_back(Err(error.to_string()));
        self
    }

    /// The requests so far, in order.
    pub fn seen(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}

impl Net for Fake {
    fn stream(&self, request: &Request) -> Result<(u16, Body), String> {
        self.seen.lock().unwrap().push(request.clone());
        self.replies.lock().unwrap().pop_front().unwrap_or_else(|| Err("the fake network has no more replies".into()))
    }
}

/// A body that waits for each piece the test sends.
struct Live {
    rx: Receiver<String>,
    left: Cursor<Vec<u8>>,
}

impl Read for Live {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = self.left.read(buf)?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            match self.rx.recv() {
                Ok(piece) => self.left = Cursor::new(piece.into_bytes()),
                Err(_) => return Ok(0),
            }
        }
    }
}
