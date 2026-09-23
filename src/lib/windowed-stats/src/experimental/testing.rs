// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use derivative::Derivative;
use fuchsia_sync::Mutex;
use std::any::{self, Any};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::experimental::clock::{Timed, Timestamp};
use crate::experimental::inspect::{InspectSender, InspectedTimeMatrix};
use crate::experimental::series::interpolation::InterpolationKind;
use crate::experimental::series::statistic::{FoldError, Metadata, SerialStatistic};
use crate::experimental::series::{SerializedBuffer, TimeMatrix, TimeMatrixFold, TimeMatrixTick};

type DynamicSample = Box<dyn Any + Send>;

#[derive(Derivative)]
#[derivative(Debug, PartialEq)]
pub enum TimeMatrixCall<T> {
    Fold(Timed<T>),
    Tick(Timestamp),
}

impl<T> TimeMatrixCall<T> {
    fn map<U, F>(self, f: F) -> TimeMatrixCall<U>
    where
        F: FnOnce(T) -> U,
    {
        match self {
            TimeMatrixCall::Fold(timed) => TimeMatrixCall::Fold(timed.map(f)),
            TimeMatrixCall::Tick(timestamp) => TimeMatrixCall::Tick(timestamp),
        }
    }
}

impl<T, E> TimeMatrixCall<Result<T, E>> {
    fn transpose(self) -> Result<TimeMatrixCall<T>, E> {
        match self {
            TimeMatrixCall::Fold(result) => match result.transpose() {
                Ok(sample) => Ok(TimeMatrixCall::Fold(sample)),
                Err(error) => Err(error),
            },
            TimeMatrixCall::Tick(timestamp) => Ok(TimeMatrixCall::Tick(timestamp)),
        }
    }
}

#[derive(Debug)]
pub struct TimeMatrixCallLog {
    calls: HashMap<String, Vec<TimeMatrixCall<DynamicSample>>>,
}

impl TimeMatrixCallLog {
    pub fn drain<T: Any + Send + Clone>(&mut self, name: &str) -> Vec<TimeMatrixCall<T>> {
        self.calls
            .remove(name)
            .unwrap_or_default()
            .into_iter()
            .map(|call| call.map(|sample| sample.downcast::<T>().map(|sample| *sample)))
            .map(TimeMatrixCall::transpose)
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_else(|_| {
                panic!(
                    "in time matrix \"{}\": failed to downcast dynamic sample of type `{}`",
                    name,
                    any::type_name::<T>()
                )
            })
    }

    pub fn as_hash_map(&self) -> &HashMap<String, Vec<TimeMatrixCall<DynamicSample>>> {
        &self.calls
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

#[derive(Clone)]
pub struct MockTimeMatrixClient {
    calls: Arc<Mutex<Vec<(String, TimeMatrixCall<DynamicSample>)>>>,
    prefix: String,
}

impl MockTimeMatrixClient {
    pub fn new() -> Self {
        Self { calls: Arc::new(Mutex::new(vec![])), prefix: String::new() }
    }
}

impl MockTimeMatrixClient {
    pub fn drain_calls(&self) -> TimeMatrixCallLog {
        let mut calls = HashMap::<_, Vec<_>>::new();
        for (name, call) in self.calls.lock().drain(..) {
            calls.entry(name).or_default().push(call);
        }
        TimeMatrixCallLog { calls }
    }
}

impl InspectSender for MockTimeMatrixClient {
    fn inspect_time_matrix<F, P>(
        &self,
        name: impl Into<String>,
        _matrix: TimeMatrix<F, P>,
    ) -> InspectedTimeMatrix<F::Sample>
    where
        TimeMatrix<F, P>: 'static + TimeMatrixFold<F::Sample> + Send,
        Metadata<F>: 'static + Send + Sync,
        F: SerialStatistic<P>,
        F::Sample: Send,
        P: InterpolationKind,
    {
        let name = format!("{}{}", self.prefix, name.into());
        let matrix = MockTimeMatrix {
            name: name.clone(),
            calls: Arc::clone(&self.calls),
            phantom: std::marker::PhantomData,
        };
        InspectedTimeMatrix::new(name, Arc::new(Mutex::new(matrix)))
    }

    fn inspect_time_matrix_with_metadata<F, P>(
        &self,
        name: impl Into<String>,
        matrix: TimeMatrix<F, P>,
        _metadata: impl Into<Metadata<F>>,
    ) -> InspectedTimeMatrix<F::Sample>
    where
        TimeMatrix<F, P>: 'static + TimeMatrixFold<F::Sample> + Send,
        Metadata<F>: 'static + Send + Sync,
        F: SerialStatistic<P>,
        F::Sample: Send,
        P: InterpolationKind,
    {
        self.inspect_time_matrix(name, matrix)
    }

    fn clone_with_child(&self, name: &str) -> Self {
        let mut new_prefix = self.prefix.clone();
        new_prefix.push_str(name);
        new_prefix.push_str("/");
        Self { calls: Arc::clone(&self.calls), prefix: new_prefix }
    }
}

struct MockTimeMatrix<T> {
    name: String,
    calls: Arc<Mutex<Vec<(String, TimeMatrixCall<DynamicSample>)>>>,
    phantom: PhantomData<fn() -> T>,
}

impl<T> TimeMatrixFold<T> for MockTimeMatrix<T>
where
    T: 'static + Send,
{
    fn fold(&mut self, sample: Timed<T>) -> Result<(), FoldError> {
        let sample = sample.map(|v| Box::new(v) as DynamicSample);
        self.calls.lock().push((self.name.clone(), TimeMatrixCall::Fold(sample)));
        Ok(())
    }
}

impl<T> TimeMatrixTick for MockTimeMatrix<T> {
    fn tick(&mut self, timestamp: Timestamp) -> Result<(), FoldError> {
        self.calls.lock().push((self.name.clone(), TimeMatrixCall::Tick(timestamp)));
        Ok(())
    }

    fn tick_and_get_buffers(
        &mut self,
        timestamp: Timestamp,
    ) -> Result<SerializedBuffer, FoldError> {
        self.tick(timestamp)?;
        Ok(SerializedBuffer { data_semantic: "mock".to_string(), data: vec![] })
    }
}
