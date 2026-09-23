// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A wrapper around a future and a guard object where the correctness of the future requires the
/// guard object to be held while the future is alive.
///
/// This is equivalent to the below code but produces a future that is almost half the size of the
/// future that rust generates: https://github.com/rust-lang/rust/issues/108906.
/// ```rust
/// let guard = acquire_guard();
/// executor.spawn(async move {
///   let _guard = guard;
///   task.await;
/// });
/// ```
#[pin_project]
pub struct FutureWithGuard<T, F, R>
where
    F: Future<Output = R> + Send + 'static,
    T: Send + 'static,
{
    // LINT.IfChange
    #[pin]
    future: F,
    // LINT.ThenChange(//src/developer/debug/zxdb/console/commands/verb_async_backtrace.cc)
    _object: T,
}

impl<T, F, R> FutureWithGuard<T, F, R>
where
    F: Future<Output = R> + Send + 'static,
    T: Send + 'static,
{
    pub fn new(object: T, future: F) -> Self {
        Self { future, _object: object }
    }
}

impl<T, F, R> Future for FutureWithGuard<T, F, R>
where
    F: Future<Output = R> + Send + 'static,
    T: Send + 'static,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().future.poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_custom_future_is_smaller() {
        async fn large_future() -> [u8; 128] {
            let held_across_await = [0; 128];
            std::future::pending::<()>().await;
            held_across_await
        }

        let object = 10u64;
        let task = large_future();
        let custom_future = FutureWithGuard::new(object, task);
        let custom_future_size = std::mem::size_of_val(&custom_future);

        let object = 10u64;
        let task = large_future();
        let rust_future = async move {
            let _object = object;
            task.await;
        };
        let rust_future_size = std::mem::size_of_val(&rust_future);

        // The large_future is 129 bytes:
        //   - 128 bytes for the array
        //   - 1 byte for the discriminant.
        //
        // The custom_future is 144 bytes:
        //   - 129 bytes for the large_future
        //   - 8 bytes for the u64 object
        //   - 7 bytes of padding
        //
        // The rust_future is 272 bytes:
        //   - 129 bytes to capture the large_future
        //   - 129 bytes to use the large_future
        //   - 8 bytes to capture the u64 object
        //   - 1 byte for the discriminant
        //   - 5 bytes of padding
        //
        // This assert only makes sure that the custom future is not bigger than the rust generated
        // future.
        //
        // If this test starts failing while updating rust, the test can safely be disabled.
        assert!(
            custom_future_size <= rust_future_size,
            "custom_future_size={custom_future_size} rust_future_size={rust_future_size}"
        );
    }
}
