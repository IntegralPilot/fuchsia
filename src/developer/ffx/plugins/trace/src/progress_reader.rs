// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io::{IsTerminal as _, Write as _};

use termion::{clear, cursor};

// A wrapper for an AsyncRead that prints progress to stderr.
pub(crate) struct ProgressReader<R> {
    inner: R,
    start_time: std::time::Instant,
    total_bytes: u64,
}

impl<R> ProgressReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self { inner, start_time: std::time::Instant::now(), total_bytes: 0 }
    }

    pub(crate) fn status_update(&self, msg: String, final_status: bool) {
        if std::io::stderr().is_terminal() {
            let ending = if final_status { "\n" } else { "" };
            eprint!(
                "{clear}{cursor_left}{msg}{ending}",
                clear = clear::CurrentLine,
                // Using Left(u16::MAX) is a bit of a hack to ensure we get
                // to the beginning of the line.
                cursor_left = cursor::Left(u16::MAX),
            );
            // Without a newline, stderr is typically line-buffered, so we need to
            // flush to ensure the message is displayed immediately.
            let _ = std::io::stderr().flush();
        } else {
            log::info!("{msg}");
        }
    }
}

impl<R: futures::io::AsyncRead + Unpin> futures::io::AsyncRead for ProgressReader<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        use std::task::Poll;
        match std::pin::Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(n)) => {
                self.total_bytes += n as u64;
                let kbytes = self.total_bytes / 1024;
                let elapsed = self.start_time.elapsed();
                let rate = if elapsed.as_secs_f64() > 0.0 {
                    kbytes as f64 / elapsed.as_secs_f64()
                } else {
                    0.0
                };
                self.status_update(format!("Read {kbytes}kB,  {rate:.2} kB/sec"), false);
                Poll::Ready(Ok(n))
            }
            other => other,
        }
    }
}
