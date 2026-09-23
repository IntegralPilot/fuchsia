// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_lock::Mutex as AsyncMutex;
use futures::future::BoxFuture;
use futures::{FutureExt, TryFutureExt};
use num::bigint::BigInt;
use num::rational::BigRational;
use std::sync::Arc;

use crate::error::Result;

use super::{PlaygroundValue, Value, ValueExt};

/// Allows implementing a [`ReplayableIterator`]. Represents the state and
/// position of a single clone of the iterator.
///
/// Essentially this is a pointer to an element to be emitted by the iterator,
/// and calling `next` retrieves a pointer to the next element.
pub trait ReplayableIteratorCursor: Send + Sync {
    /// Return a cursor pointing to the next element in this stream.
    fn next(
        self: Arc<Self>,
    ) -> (BoxFuture<'static, Result<Option<Value>>>, Arc<dyn ReplayableIteratorCursor>);
}

impl<T: ReplayableIteratorCursor + Sized + 'static> From<T> for ReplayableIterator {
    fn from(cursor: T) -> ReplayableIterator {
        ReplayableIterator(Arc::new(cursor))
    }
}

/// An iterator that can be rewound and replayed, and copied, with each copy
/// able to occupy a different head point in the iterator.
#[derive(Clone)]
pub struct ReplayableIterator(Arc<dyn ReplayableIteratorCursor + 'static>);

impl ReplayableIterator {
    /// Get the next value from this iterator.
    pub fn next(&mut self) -> BoxFuture<'static, Result<Option<Value>>> {
        let (fut, new) = Arc::clone(&self.0).next();
        self.0 = new;
        fut
    }

    /// Map this iterator as would [`std::iter::Iterator::map`].
    pub fn map<F: Fn(Value) -> BoxFuture<'static, Result<Value>> + 'static + Send + Sync>(
        self,
        f: F,
    ) -> ReplayableIterator {
        ReplayableIterator(Arc::new(MappedReplayableIteratorCursor(self.0, Arc::new(f))))
    }
}

impl TryFrom<Value> for ReplayableIterator {
    type Error = ();

    fn try_from(v: Value) -> Result<ReplayableIterator, ()> {
        match v {
            Value::OutOfLine(PlaygroundValue::Iterator(i)) => Ok(i),
            Value::List(l) => {
                Ok(VecReplayableIteratorCursor(0, Arc::new(AsyncMutex::new(l))).into())
            }
            _ => Err(()),
        }
    }
}

/// Implement [`ReplayableIterator`] on top of a vector of values.
struct VecReplayableIteratorCursor(usize, Arc<AsyncMutex<Vec<Value>>>);

impl ReplayableIteratorCursor for VecReplayableIteratorCursor {
    fn next(
        self: Arc<Self>,
    ) -> (BoxFuture<'static, Result<Option<Value>>>, Arc<dyn ReplayableIteratorCursor>) {
        let this = Arc::clone(&self);
        (
            async move { Ok(this.1.lock().await.get_mut(this.0).map(Value::duplicate)) }.boxed(),
            Arc::new(VecReplayableIteratorCursor(self.0 + 1, Arc::clone(&self.1))),
        )
    }
}

/// Internals of a ReplayableIterator which is generated from a range.
pub struct RangeCursor {
    pub start: BigInt,
    pub end: Option<BigInt>,
    pub is_inclusive: bool,
}

impl ReplayableIteratorCursor for RangeCursor {
    fn next(
        self: Arc<Self>,
    ) -> (BoxFuture<'static, Result<Option<Value>>>, Arc<dyn ReplayableIteratorCursor>) {
        let done = if let Some(end) = &self.end {
            self.start > *end || (self.start == *end && !self.is_inclusive)
        } else {
            false
        };

        if done {
            (async move { Ok(None) }.boxed(), self)
        } else {
            let start = self.start.clone();
            (
                async move {
                    Ok(Some(Value::OutOfLine(PlaygroundValue::Num(BigRational::from_integer(
                        start,
                    )))))
                }
                .boxed(),
                Arc::new(RangeCursor {
                    start: self.start.clone() + 1,
                    end: self.end.clone(),
                    is_inclusive: self.is_inclusive,
                }),
            )
        }
    }
}

/// Implements the [`ReplayableIterator`] returned by [`ReplayableIterator::map`].
struct MappedReplayableIteratorCursor(
    Arc<dyn ReplayableIteratorCursor + 'static>,
    Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value>> + 'static + Send + Sync>,
);

impl ReplayableIteratorCursor for MappedReplayableIteratorCursor {
    fn next(
        self: Arc<Self>,
    ) -> (BoxFuture<'static, Result<Option<Value>>>, Arc<dyn ReplayableIteratorCursor>) {
        let (fut, next_inner) = Arc::clone(&self.0).next();
        let func = Arc::clone(&self.1);
        let func_ref = Arc::clone(&self.1);
        let fut = fut
            .and_then(move |x| async move {
                match x {
                    r @ None => Ok(r),
                    Some(v) => func_ref(v).await.map(Some),
                }
            })
            .boxed();
        (fut, Arc::new(MappedReplayableIteratorCursor(next_inner, func)))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Test an iterator. The iterator should yield 5, 6, 7, and 8 in order. If
    /// `check_end` is true it must not yield anything after that.
    async fn test_iter(mut iter: ReplayableIterator, check_end: bool) {
        let mut iter_mapped = iter.clone().map(|x| {
            async move {
                let x = x.try_big_num().unwrap();
                let x = x + BigRational::from_integer(2.into());
                Ok(Value::OutOfLine(PlaygroundValue::Num(x)))
            }
            .boxed()
        });

        let a = iter.next().await.unwrap().unwrap().try_big_num().unwrap();
        assert_eq!(a, BigRational::from_integer(5.into()));
        let a = iter.next().await.unwrap().unwrap().try_big_num().unwrap();
        assert_eq!(a, BigRational::from_integer(6.into()));
        let a = iter.next().await.unwrap().unwrap().try_big_num().unwrap();
        assert_eq!(a, BigRational::from_integer(7.into()));
        let a = iter_mapped.next().await.unwrap().unwrap().try_big_num().unwrap();
        assert_eq!(a, BigRational::from_integer(7.into()));
        let a = iter_mapped.next().await.unwrap().unwrap().try_big_num().unwrap();
        assert_eq!(a, BigRational::from_integer(8.into()));
        let a = iter.next().await.unwrap().unwrap().try_big_num().unwrap();
        assert_eq!(a, BigRational::from_integer(8.into()));
        assert!(!check_end || iter.next().await.unwrap().is_none());
        let a = iter_mapped.next().await.unwrap().unwrap().try_big_num().unwrap();
        assert_eq!(a, BigRational::from_integer(9.into()));
        let a = iter_mapped.next().await.unwrap().unwrap().try_big_num().unwrap();
        assert_eq!(a, BigRational::from_integer(10.into()));
        assert!(!check_end || iter_mapped.next().await.unwrap().is_none());
    }

    #[fuchsia::test]
    async fn limited_exclusive() {
        let iter = RangeCursor { start: 5.into(), end: Some(9.into()), is_inclusive: false };

        test_iter(ReplayableIterator::from(iter), true).await;
    }

    #[fuchsia::test]
    async fn limited_inclusive() {
        let iter = RangeCursor { start: 5.into(), end: Some(8.into()), is_inclusive: true };

        test_iter(ReplayableIterator::from(iter), true).await;
    }

    #[fuchsia::test]
    async fn unlimited() {
        let iter = RangeCursor { start: 5.into(), end: None, is_inclusive: true };

        test_iter(ReplayableIterator::from(iter), false).await;
    }

    #[fuchsia::test]
    async fn list() {
        let iter = ReplayableIterator::try_from(Value::List(vec![
            Value::U8(5),
            Value::U8(6),
            Value::U8(7),
            Value::U8(8),
        ]))
        .unwrap();
        test_iter(iter, true).await;
    }
}
