//! A general version async Notify, like `tokio` Notify but can work with any async runtime.

use std::future::Future;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, ready};

use event_listener::{Event, EventListener, listener};
use futures_core::Stream;
use pin_project_lite::pin_project;

/// Notify a single task to wake up.
///
/// `Notify` provides a basic mechanism to notify a single task of an event.
/// `Notify` itself does not carry any data. Instead, it is to be used to signal
/// another task to perform an operation.
///
/// If [`notify()`] is called **before** [`notified().await`], then the next call to
/// [`notified().await`] will complete immediately, consuming one permit. Permits
/// accumulate: each call to [`notify()`] or [`notify_n()`] adds permits that can
/// be consumed by subsequent [`notified().await`] calls.
///
/// [`notify()`]: Notify::notify
/// [`notified().await`]: Notify::notified()
///
/// # Examples
///
/// Basic usage.
///
/// ```
/// use std::sync::Arc;
/// use async_notify::Notify;
///
/// async_global_executor::block_on(async {
///    let notify = Arc::new(Notify::new());
///    let notify2 = notify.clone();
///
///    async_global_executor::spawn(async move {
///        notify2.notify();
///        println!("sent notification");
///    })
///    .detach();
///
///    println!("received notification");
///    notify.notified().await;
/// })
/// ```
#[derive(Debug, Default)]
pub struct Notify {
    count: AtomicUsize,
    event: Event,
}

/// Like tokio Notify, this is a runtime independent Notify.
impl Notify {
    /// Create a [`Notify`]
    pub const fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
            event: Event::new(),
        }
    }

    /// Notifies a waiting task
    ///
    /// Adds one permit to this `Notify`. If a task is currently waiting on
    /// [`notified().await`], that task will be woken and complete. Otherwise,
    /// the permit is stored and the next call to [`notified().await`] will
    /// complete immediately. Permits accumulate across multiple `notify()` calls.
    ///
    /// [`notified().await`]: Notify::notified()
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use async_notify::Notify;
    ///
    /// async_global_executor::block_on(async {
    ///    let notify = Arc::new(Notify::new());
    ///    let notify2 = notify.clone();
    ///
    ///    async_global_executor::spawn(async move {
    ///        notify2.notify();
    ///        println!("sent notification");
    ///    })
    ///    .detach();
    ///
    ///    println!("received notification");
    ///    notify.notified().await;
    /// })
    /// ```
    #[inline]
    pub fn notify(&self) {
        self.notify_n(NonZeroUsize::new(1).unwrap())
    }

    /// Grants `n` permits and notifies up to `n` waiting tasks.
    ///
    /// Adds `n` permits to this `Notify`. If there are tasks currently waiting
    /// on [`notified().await`], up to `n` of them will be woken and complete,
    /// each consuming one permit. If no tasks are waiting, the permits are
    /// stored and the next up to `n` calls to [`notified().await`] will complete
    /// immediately.
    ///
    /// This is a generalization of [`notify()`] which is equivalent to
    /// `notify_n(NonZeroUsize::MIN)`.
    ///
    /// [`notified().await`]: Notify::notified()
    /// [`notify()`]: Notify::notify
    #[inline]
    pub fn notify_n(&self, n: NonZeroUsize) {
        let n = n.get();
        self.count.fetch_add(n, Ordering::Release);
        self.event.notify(n);
    }

    /// Wakes up to `n` waiting tasks to compete for existing permits.
    ///
    /// Unlike [`notify_n()`], this does **not** add any permits. It only wakes
    /// up to `n` tasks that are waiting on [`notified().await`]. Those tasks
    /// will then compete for whatever permits are currently available. At most
    /// one task can consume each available permit; the rest will wait for the
    /// next notification.
    ///
    /// Use this when you want to wake multiple waiters to race for a single
    /// resource (e.g. thundering herd mitigation).
    ///
    /// [`notified().await`]: Notify::notified()
    /// [`notify_n()`]: Notify::notify_n
    #[inline]
    pub fn notify_waiters(&self, n: NonZeroUsize) {
        self.event.notify(n.get());
    }

    /// Wait for a notification.
    ///
    /// Each `Notify` value holds a number of permits. If a permit is available
    /// from an earlier call to [`notify()`] or [`notify_n()`], then
    /// `notified().await` will complete immediately, consuming one permit.
    /// Otherwise, `notified().await` waits for a permit to be made available.
    ///
    /// This method is cancel safety.
    ///
    /// [`notify()`]: Notify::notify
    /// [`notify_n()`]: Notify::notify_n
    #[inline]
    pub async fn notified(&self) {
        loop {
            if self.fast_path() {
                return;
            }

            listener!(self.event => listener);

            if self.fast_path() {
                return;
            }

            listener.await;
        }
    }

    fn fast_path(&self) -> bool {
        // to support old version rustc
        #[allow(deprecated)]
        self.count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |c| c.checked_sub(1))
            .is_ok()
    }
}

pin_project! {
    /// A [`Stream`](Stream) [`Notify`] wrapper
    pub struct NotifyStream<T: Deref<Target=Notify>> {
        #[pin]
        notify: T,
        listener: Option<EventListener>,
    }
}

impl<T: Deref<Target = Notify>> NotifyStream<T> {
    /// Create [`NotifyStream`] from `T`
    pub const fn new(notify: T) -> Self {
        Self {
            notify,
            listener: None,
        }
    }

    /// acquire the inner [`T`]
    pub fn into_inner(self) -> T {
        self.notify
    }
}

impl<T: Deref<Target = Notify>> AsRef<Notify> for NotifyStream<T> {
    fn as_ref(&self) -> &Notify {
        self.notify.deref()
    }
}

impl<T: Deref<Target = Notify>> Stream for NotifyStream<T> {
    type Item = ();

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let notify = this.notify.deref();

        loop {
            if notify.fast_path() {
                *this.listener = None;

                return Poll::Ready(Some(()));
            }

            match this.listener.as_mut() {
                None => {
                    let listener = notify.event.listen();
                    *this.listener = Some(listener);
                }
                Some(listener) => {
                    ready!(Pin::new(listener).poll(cx));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::{FutureExt, StreamExt, select};

    use super::*;

    #[test]
    fn test() {
        async_global_executor::block_on(async {
            let notify = Arc::new(Notify::new());
            let notify2 = notify.clone();

            async_global_executor::spawn(async move {
                notify2.notify();
                println!("sent notification");
            })
            .detach();

            println!("received notification");
            notify.notified().await;
        })
    }

    #[test]
    fn test_multi_notify() {
        async_global_executor::block_on(async {
            let notify = Arc::new(Notify::new());
            let notify2 = notify.clone();

            // 2 permits
            notify.notify();
            notify.notify();

            // First completes (1 permit consumed)
            select! {
                _ = notify2.notified().fuse() => {}
                default => unreachable!("there should be notified")
            }

            // Second completes (1 permit consumed)
            select! {
                _ = notify2.notified().fuse() => {}
                default => unreachable!("there should be notified")
            }

            // No permits left, third would block
            select! {
                _ = notify2.notified().fuse() => unreachable!("there should not be notified"),
                default => {}
            }

            notify.notify();

            // Third completes
            select! {
                _ = notify2.notified().fuse() => {}
                default => unreachable!("there should be notified")
            }
        })
    }

    #[test]
    fn test_notify_n() {
        async_global_executor::block_on(async {
            let notify = Arc::new(Notify::new());
            let notify2 = notify.clone();

            notify.notify_n(3.try_into().unwrap());

            for _ in 0..3 {
                select! {
                    _ = notify2.notified().fuse() => {}
                    default => unreachable!("there should be notified")
                }
            }

            select! {
                _ = notify2.notified().fuse() => unreachable!("there should not be notified"),
                default => {}
            }
        })
    }

    #[test]
    fn test_notify_waiters() {
        async_global_executor::block_on(async {
            let notify = Arc::new(Notify::new());
            let notify2 = notify.clone();
            let notify3 = notify.clone();

            let t1 = async_global_executor::spawn(async move {
                notify2.notified().await;
            });
            let t2 = async_global_executor::spawn(async move {
                notify3.notified().await;
            });

            // Give tasks time to start waiting
            async_global_executor::spawn(async {}).await;

            // 1 permit, wake 2 waiters - only 1 can complete
            notify.notify();
            notify.notify_waiters(NonZeroUsize::new(2).unwrap());

            // One completes. Add permit for the other.
            notify.notify();

            t1.await;
            t2.await;
        })
    }

    #[test]
    fn stream() {
        async_global_executor::block_on(async {
            let notify = Arc::new(Notify::new());
            let mut notify_stream = NotifyStream::new(notify.clone());

            async_global_executor::spawn(async move {
                notify.notify();
                println!("sent notification");
            })
            .detach();

            notify_stream.next().await.unwrap();
        })
    }
}
