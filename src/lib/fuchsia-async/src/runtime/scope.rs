// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use super::implementation::scope::{Scope, ScopeActiveGuard, ScopeHandle};

#[cfg(target_os = "fuchsia")]
pub use super::implementation::scope::{Join, ScopeStream, Spawnable};

#[cfg(test)]
mod tests {
    use crate::{yield_now, JoinHandle, Scope, TestExecutor};
    use assert_matches::assert_matches;
    use futures::future::join_all;
    use futures::stream::FuturesUnordered;
    use futures::{join, FutureExt, StreamExt};
    use std::future::{pending, poll_fn, Future};
    use std::pin::{pin, Pin};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll};

    // Tokio doesn't have the equivalent of poll_until_stalled. This has a crude workaround.
    async fn poll_until_stalled<T>(future: impl Future<Output = T> + Unpin) -> Poll<T> {
        #[cfg(target_os = "fuchsia")]
        {
            TestExecutor::poll_until_stalled(future).await
        }

        #[cfg(not(target_os = "fuchsia"))]
        {
            let mut future = future;
            for _ in 0..10 {
                if let Poll::Ready(result) = futures::poll!(&mut future) {
                    return Poll::Ready(result);
                }
                crate::yield_now().await;
            }
            Poll::Pending
        }
    }

    fn new_scope() -> Scope {
        #[cfg(target_os = "fuchsia")]
        {
            crate::EHandle::local().global_scope().new_child()
        }

        #[cfg(not(target_os = "fuchsia"))]
        {
            Scope::new_with_name("test")
        }
    }

    fn abort_task(task: JoinHandle<()>) {
        #[cfg(target_os = "fuchsia")]
        {
            drop(task.abort());
        }

        #[cfg(not(target_os = "fuchsia"))]
        {
            task.abort();
        }
    }

    fn run_with_test_executor(fut: impl Future) {
        #[cfg(target_os = "fuchsia")]
        assert!(TestExecutor::new().run_until_stalled(&mut pin!(fut)).is_ready());

        #[cfg(not(target_os = "fuchsia"))]
        TestExecutor::new().run_singlethreaded(fut);
    }

    #[test]
    fn on_no_tasks() {
        run_with_test_executor(async {
            let scope = new_scope();
            let _task1 = scope.spawn(std::future::ready(()));
            let task2 = scope.spawn(pending::<()>());

            // A guard shouldn't stop on_no_tasks from working.
            let _guard = scope.active_guard().unwrap();

            let mut on_no_tasks = pin!(scope.on_no_tasks());

            assert!(poll_until_stalled(&mut on_no_tasks).await.is_pending());

            abort_task(task2);

            let on_no_tasks2 = pin!(scope.on_no_tasks());
            let on_no_tasks3 = pin!(scope.on_no_tasks());

            assert_matches!(
                poll_until_stalled(&mut join_all([on_no_tasks, on_no_tasks2, on_no_tasks3])).await,
                Poll::Ready(_)
            );
        });
    }

    #[test]
    fn on_no_tasks_and_guards() {
        #[derive(Debug)]
        enum Pass {
            DropTaskFirst,
            DropGuardFirst,
            WakeTaskWithGuardAndThenDrop,
            WakeTaskWithGuardAndThenQuit,
        }

        for pass in [
            Pass::DropTaskFirst,
            Pass::DropGuardFirst,
            Pass::WakeTaskWithGuardAndThenDrop,
            Pass::WakeTaskWithGuardAndThenQuit,
        ] {
            run_with_test_executor(async {
                let scope = new_scope();
                let _task1 = scope.spawn(std::future::ready(()));
                let should_quit = Arc::new(AtomicBool::new(false));
                let should_quit_clone = should_quit.clone();
                let task2 = scope.spawn(poll_fn(move |_| {
                    if should_quit_clone.load(Ordering::Relaxed) {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                }));
                let guard = scope.active_guard().unwrap();

                let on_no_tasks = pin!(scope.on_no_tasks_and_guards());

                // Use FuturesUnordered so that it uses its own waker.
                let mut futures_unordered = FuturesUnordered::new();
                futures_unordered.push(on_no_tasks);

                // Pending because there's task2 and a guard.
                assert!(poll_until_stalled(&mut futures_unordered.next()).await.is_pending());

                match pass {
                    Pass::DropTaskFirst => {
                        // Drop the task first.
                        abort_task(task2);

                        // Still pending because there's a guard.
                        assert!(poll_until_stalled(&mut futures_unordered.next())
                            .await
                            .is_pending());

                        drop(guard);
                    }
                    Pass::DropGuardFirst => {
                        // Drop the guard first.
                        drop(guard);

                        // Still pending because there's a guard.
                        assert!(poll_until_stalled(&mut futures_unordered.next())
                            .await
                            .is_pending());

                        abort_task(task2);
                    }
                    Pass::WakeTaskWithGuardAndThenDrop => {
                        // Drop the guard first.
                        drop(guard);

                        // This time wake the second task with an active guard, but then immediately
                        // drop the task.
                        scope.wake_all_with_active_guard();
                        abort_task(task2);
                    }
                    Pass::WakeTaskWithGuardAndThenQuit => {
                        // Drop the guard first.
                        drop(guard);

                        // Wake the task with an active guard, and make it quit.
                        should_quit.store(true, Ordering::Relaxed);
                        scope.wake_all_with_active_guard();
                    }
                }

                let on_no_tasks2 = pin!(scope.on_no_tasks_and_guards());
                let on_no_tasks3 = pin!(scope.on_no_tasks_and_guards());

                assert_matches!(
                    poll_until_stalled(&mut pin!(async {
                        join!(futures_unordered.next(), on_no_tasks2, on_no_tasks3);
                    }))
                    .await,
                    Poll::Ready(_),
                    "pass={pass:?}",
                );
            });
        }
    }

    #[test]
    fn wake_all_with_active_guard() {
        run_with_test_executor(async {
            let scope = new_scope();

            let poll_count = Arc::new(AtomicU64::new(0));

            struct PollCounter(Arc<AtomicU64>);

            impl Future for PollCounter {
                type Output = ();
                fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
                    self.0.fetch_add(1, Ordering::Relaxed);
                    Poll::Pending
                }
            }

            scope.spawn(PollCounter(poll_count.clone()));
            scope.spawn(PollCounter(poll_count.clone()));

            let _ = poll_until_stalled(&mut pending::<()>()).await;

            let mut start_count = poll_count.load(Ordering::Relaxed);

            for _ in 0..2 {
                scope.wake_all_with_active_guard();
                let _ = poll_until_stalled(&mut pending::<()>()).await;
                assert_eq!(poll_count.load(Ordering::Relaxed), start_count + 2);
                start_count += 2;
            }

            // Wake, then cancel the scope and verify the tasks still get polled.
            scope.wake_all_with_active_guard();
            let mut done = pin!(scope.cancel());
            let _ = poll_until_stalled(&mut pending::<()>()).await;
            assert_eq!(poll_count.load(Ordering::Relaxed), start_count + 2);
            assert_eq!(poll_until_stalled(&mut done).await, Poll::Ready(()));
        });
    }

    #[test]
    fn active_guard_holds_cancellation() {
        run_with_test_executor(async {
            let scope = new_scope();
            let guard = scope.active_guard().expect("acquire guard");
            scope.spawn(futures::future::pending());
            let mut join = pin!(scope.cancel());
            yield_now().await;
            assert!(join.as_mut().now_or_never().is_none());
            drop(guard);
            join.await;
        });
    }

    #[test]
    fn detach() {
        run_with_test_executor(async {
            let scope = Scope::new_with_name("detach");
            let guard = scope.active_guard().expect("acquire guard");
            let shared = Arc::new(());
            let shared_copy = shared.clone();
            scope.spawn(async move {
                let _shared = shared_copy;
                let _guard = guard;
                let () = futures::future::pending().await;
            });
            scope.detach();
            yield_now().await;
            assert_eq!(Arc::strong_count(&shared), 2);
        });
    }

    #[test]
    fn abort() {
        run_with_test_executor(async {
            let scope = Scope::new_with_name("abort");
            let guard = scope.active_guard().expect("acquire guard");
            let shared = Arc::new(());
            let shared_copy = shared.clone();
            scope.spawn(async move {
                let _shared = shared_copy;
                let _guard = guard;
                let () = futures::future::pending().await;
            });
            scope.clone().abort().await;
            assert_eq!(Arc::strong_count(&shared), 1);
        });
    }
}
