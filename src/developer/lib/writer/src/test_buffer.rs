// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use event_listener::Event;
use futures::AsyncWrite;
use std::cell::RefCell;
use std::io::Write;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

/// Provides shared memory buffers (in the form of [`TestBuffer`]s for stdout
/// and stderr that can be used with implementations of [`crate::ToolIO`] to
/// test input and output behaviour at runtime.
#[derive(Default, Debug)]
pub struct TestBuffers {
    pub stdout: TestBuffer,
    pub stderr: TestBuffer,
}

/// Provides a shared memory buffer that can be cloned that can be used with
/// implementations of [`crate::ToolIO`] to test input and output behaviour
/// at runtime.
#[derive(Clone, Default, Debug)]
pub struct TestBuffer {
    inner: Rc<RefCell<Vec<u8>>>,
    event: Rc<Event>,
}

impl TestBuffers {
    /// Destroy `self` and return the standard output and error buffers as byte
    /// arrays.
    pub fn into_stdio(self) -> (Vec<u8>, Vec<u8>) {
        let stdout = self.stdout.into_inner();
        let stderr = self.stderr.into_inner();
        (stdout, stderr)
    }

    /// Destroy `self` and return the standard output and error buffers as strings.
    pub fn into_strings(self) -> (String, String) {
        let stdout = self.stdout.into_string();
        let stderr = self.stderr.into_string();
        (stdout, stderr)
    }

    /// Destroy `self` and return the standard output buffer as a byte
    /// array.
    pub fn into_stdout(self) -> Vec<u8> {
        self.stdout.into_inner()
    }

    /// Destroy `self` and return the standard output buffer as a string.
    pub fn into_stdout_str(self) -> String {
        self.stdout.into_string()
    }

    /// Destroy `self` and return the standard error buffer as a byte
    /// array.
    pub fn into_stderr(self) -> Vec<u8> {
        self.stderr.into_inner()
    }

    /// Destroy `self` and return the standard error buffer as a string.
    pub fn into_stderr_str(self) -> String {
        self.stderr.into_string()
    }
}

impl TestBuffer {
    /// Destroys self and returns the inner buffer as a vector.
    pub fn into_inner(self) -> Vec<u8> {
        self.inner.take().into()
    }

    /// Destroys self and returns the inner buffer as a string.
    pub fn into_string(self) -> String {
        String::from_utf8(self.into_inner()).expect("Valid unicode on output string")
    }

    /// Waits for the buffer to be written to.  This will not return immediately if data is already
    /// present so this should be called *after* calling `into_inner` or `into_string` and not
    /// finding the desired results.
    pub async fn wait_ready(&self) {
        self.event.listen().await;
    }
}

impl Write for TestBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.event.notify(usize::MAX);
        self.inner.borrow_mut().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.borrow_mut().flush()
    }
}

impl AsyncWrite for TestBuffer {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let size = self.inner.borrow_mut().write(buf);
        Poll::Ready(size)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.inner.borrow_mut().flush() {
            Ok(_) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
