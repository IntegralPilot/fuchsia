// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::{Endpoint, Interface, ZeroPacket};
use futures::io::{AsyncRead, AsyncWrite};
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use tokio::sync::RwLock;

/// Maximum size of a single bulk URB submitted to Linux usbfs (512 KiB).
///
/// This is an engineering heuristic: it maximizes xHCI DMA burst throughput
/// while remaining safely under kernel memory limits.
const MAX_USBFS_BULK_WRITE_SIZE: usize = 512 * 1024;

/// Maximum number of bulk URBs queued in flight concurrently (16 URBs = 8 MiB).
///
/// This heuristic keeps the host controller's DMA hardware ring continuously saturated
/// without stalling between chunks, while remaining well within the Linux usbfs memory
/// budget (`usbfs_memory_mb`, default 16 MiB) and the interface URB pool limit (32).
const MAX_IN_FLIGHT_URBS: usize = 16;
const MAX_WRITE_BUFFER_SIZE: usize = MAX_USBFS_BULK_WRITE_SIZE * MAX_IN_FLIGHT_URBS;

fn is_out_of_memory(err: &crate::Error) -> bool {
    match err {
        crate::Error::IOError(io_err) => {
            io_err.raw_os_error() == Some(libc::ENOMEM)
                || io_err.kind() == std::io::ErrorKind::OutOfMemory
        }
        _ => false,
    }
}

/// Wraps an `Interface` and impls AsyncRead and AsyncWrite and reads and
/// writes to the appropriate In/Out endpoints of the interface
pub struct BulkInterface {
    inner: Arc<Interface>,
    guard: Arc<RwLock<()>>,
    read_future: Option<Pin<Box<dyn Future<Output = std::io::Result<Box<[u8]>>> + Send>>>,
    write_future: Option<Pin<Box<dyn Future<Output = std::io::Result<usize>> + Send>>>,
}

impl std::fmt::Debug for BulkInterface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("BulkInterface")
            .field("read_future", &self.read_future.is_some())
            .field("write_future", &self.write_future.is_some())
            .finish()
    }
}

impl BulkInterface {
    pub fn new(inner: Interface) -> Self {
        Self {
            inner: Arc::new(inner),
            guard: Arc::new(RwLock::new(())),
            read_future: None,
            write_future: None,
        }
    }
}

impl AsyncRead for BulkInterface {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        mut buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        log::debug!("BulkInterface Poll read: {:?}", self);
        if self.read_future.is_none() {
            let inner_ref = self.inner.clone();
            let guard_ref = self.guard.clone();
            let mut buffer = buf.to_vec().into_boxed_slice();
            let read_future = async move {
                // Get the bulk in interface
                for endpoint in inner_ref.endpoints() {
                    if let Endpoint::BulkIn(bie) = endpoint {
                        let _guard = guard_ref.read().await;
                        log::debug!(
                            "Found bulk in endpoint, reading to buf of len: {}",
                            buffer.len()
                        );
                        bie.read(&mut buffer).await.map_err(|e| {
                            std::io::Error::new(
                                std::io::ErrorKind::Other,
                                format!("Error reading from bulk endpoint: {}", e),
                            )
                        })?;
                        return Ok(buffer);
                    }
                }
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "No bulk in endpoint found"))
            };

            self.read_future = Some(Box::pin(read_future));
        }

        let future = self.read_future.as_mut().unwrap();
        match future.as_mut().poll(cx) {
            Poll::Ready(Ok(buffer)) => {
                self.read_future = None;
                Poll::Ready(buf.write(&buffer))
            }
            Poll::Ready(Err(e)) => {
                self.read_future = None;
                Poll::Ready(Err(e))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for BulkInterface {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        if self.write_future.is_none() {
            let Ok(guard) = self.guard.clone().try_write_owned() else {
                let guard_ref = self.guard.clone();
                let wait_lock = async move {
                    let _g = guard_ref.write_owned().await;
                    Ok(0)
                };
                return match self.write_future.insert(Box::pin(wait_lock)).as_mut().poll(cx) {
                    Poll::Ready(Ok(0)) => {
                        self.write_future = None;
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                    Poll::Ready(Ok(s)) => Poll::Ready(Ok(s)),
                    Poll::Ready(Err(e)) => {
                        self.write_future = None;
                        Poll::Ready(Err(e))
                    }
                    Poll::Pending => Poll::Pending,
                };
            };

            let Some(boe) = self.inner.endpoints().into_iter().find_map(|endpoint| {
                if let Endpoint::BulkOut(boe) = endpoint { Some(boe) } else { None }
            }) else {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "No bulk out endpoint found",
                )));
            };

            let to_write = std::cmp::min(buf.len(), MAX_WRITE_BUFFER_SIZE);
            let mut in_flight = std::collections::VecDeque::new();
            let mut bytes_submitted = 0;

            // Pipeline up to MAX_IN_FLIGHT_URBS (or until the interface URB pool is
            // exhausted) directly from `buf` into kernel URBs without intermediate copies.
            // If the buffer is larger than this batch or the pool runs out of URBs, we stop
            // submitting and await the in-flight requests. Returning `Poll::Ready(Ok(bytes_submitted))`
            // fulfills standard `AsyncWrite::poll_write` partial-write semantics, allowing
            // the caller (e.g. `write_all`) to re-invoke `poll_write` for subsequent chunks.
            for chunk in buf[..to_write].chunks(MAX_USBFS_BULK_WRITE_SIZE) {
                match boe.try_write_defer_wait(chunk, ZeroPacket::DoNotSend) {
                    Ok(Some(wait_fut)) => {
                        in_flight.push_back(wait_fut);
                        bytes_submitted += chunk.len();
                        if in_flight.len() >= MAX_IN_FLIGHT_URBS {
                            break;
                        }
                    }
                    Ok(None) => {
                        // Pool has no free URBs right now.
                        break;
                    }
                    Err(e) => {
                        if !in_flight.is_empty() && is_out_of_memory(&e) {
                            log::debug!(
                                "Kernel usbfs memory saturated after submitting {} bytes across {} in-flight URBs; draining before submitting more",
                                bytes_submitted,
                                in_flight.len()
                            );
                            break;
                        }
                        log::warn!("Error submitting bulk URB: {}", e);
                        let err_msg = if is_out_of_memory(&e) {
                            // In this scenario we should consider increasing
                            // with: echo 16 | sudo tee /sys/module/usbcore/parameters/usbfs_memory_mb)
                            format!(
                                "Error submitting to bulk endpoint: {e} Linux usbfs_memory_mb exhausted"
                            )
                        } else {
                            format!("Error submitting to bulk endpoint: {e}")
                        };
                        return Poll::Ready(Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            err_msg,
                        )));
                    }
                }
            }

            if in_flight.is_empty() {
                // All URBs are in flight; yield so reaper thread can reap completed ones
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }

            log::debug!(
                "Submitted {} bytes across {} pipelined URBs directly from buffer",
                bytes_submitted,
                in_flight.len()
            );

            let write_future = async move {
                let _guard = guard;
                while let Some(fut) = in_flight.pop_front() {
                    fut.await.map_err(|e| {
                        log::warn!("Error awaiting bulk URB completion: {}", e);
                        std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!("Error writing to bulk endpoint: {}", e),
                        )
                    })?;
                }
                Ok(bytes_submitted)
            };

            self.write_future = Some(Box::pin(write_future));
        }

        let future = self.write_future.as_mut().unwrap();
        match future.as_mut().poll(cx) {
            Poll::Ready(Ok(size)) => {
                self.write_future = None;
                Poll::Ready(Ok(size))
            }
            Poll::Ready(Err(e)) => {
                log::debug!("Poll write: error: {:?}", e);
                self.write_future = None;
                Poll::Ready(Err(e))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_out_of_memory() {
        let enomem_err = crate::Error::IOError(std::io::Error::from_raw_os_error(libc::ENOMEM));
        assert!(is_out_of_memory(&enomem_err));

        let other_err = crate::Error::IOError(std::io::Error::from_raw_os_error(libc::EINVAL));
        assert!(!is_out_of_memory(&other_err));

        let short_err = crate::Error::ShortWrite(100, 50);
        assert!(!is_out_of_memory(&short_err));
    }
}
