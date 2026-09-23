// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Type that wraps a closure for completing an operation.
///
/// Calling `Completer::reply()` forwards the status to the wrapped closure. Otherwise,
/// dropping a `Completer` indicates failure with a `zx::Status::BAD_STATE` status.
pub struct Completer<F>
where
    F: FnOnce(zx::sys::zx_status_t),
{
    completer: Option<F>,
}

// SAFETY: It's safe to use Completer on any thread because the caller of the constructor
// promises the inner completer is safe to send to another thread.
unsafe impl<F> Send for Completer<F> where F: FnOnce(zx::sys::zx_status_t) {}

impl<F> Completer<F>
where
    F: FnOnce(zx::sys::zx_status_t),
{
    /// # Safety
    ///
    /// Caller promises the provided completer is safe to send to another thread.
    /// In some cases, providing a completer that implements Send is difficult.
    /// For example, a closure that captures a pointer does not implement Send.
    pub unsafe fn new_unchecked(completer: F) -> Self {
        Self { completer: Some(completer) }
    }

    pub fn reply(mut self, status: Result<(), zx::Status>) {
        let completer = match self.completer.take() {
            None => unreachable!(),
            Some(completer) => completer,
        };
        completer(zx::Status::result_into_raw(status))
    }
}

impl<F> Completer<F>
where
    F: FnOnce(zx::sys::zx_status_t) + Send,
{
    pub fn new(completer: F) -> Self {
        Self { completer: Some(completer) }
    }
}

impl<F> Drop for Completer<F>
where
    F: FnOnce(zx::sys::zx_status_t),
{
    fn drop(&mut self) {
        if let Some(completer) = self.completer.take() {
            completer(zx::Status::BAD_STATE.into_raw())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::oneshot;

    #[test]
    fn reply_with_ok() {
        let (sender, mut receiver) = oneshot::channel::<zx::sys::zx_status_t>();
        let completer = Completer::new(move |status| {
            sender.send(status).expect("Failed to send result.");
        });
        completer.reply(Ok(()));
        assert_eq!(Ok(Some(zx::sys::ZX_OK)), receiver.try_recv());
    }

    #[test]
    fn reply_with_error() {
        let (sender, mut receiver) = oneshot::channel::<zx::sys::zx_status_t>();
        let completer = Completer::new(move |status| {
            sender.send(status).expect("Failed to send result.");
        });
        completer.reply(Err(zx::Status::NO_RESOURCES));
        assert_eq!(Ok(Some(zx::Status::NO_RESOURCES.into_raw())), receiver.try_recv());
    }

    #[test]
    fn reply_with_error_when_dropped() {
        let (sender, mut receiver) = oneshot::channel::<zx::sys::zx_status_t>();
        let completer = Completer::new(move |status| {
            sender.send(status).expect("Failed to send result.");
        });
        drop(completer);
        assert_eq!(Ok(Some(zx::Status::BAD_STATE.into_raw())), receiver.try_recv());
    }
}
