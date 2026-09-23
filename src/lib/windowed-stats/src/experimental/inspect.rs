// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use derivative::Derivative;
use fuchsia_inspect::{Inspector, Node as InspectNode};
use fuchsia_sync::Mutex;
use futures::FutureExt as _;
use log::warn;
use std::sync::Arc;

use crate::experimental::clock::{Timed, Timestamp};
use crate::experimental::series::interpolation::InterpolationKind;
use crate::experimental::series::statistic::{FoldError, Metadata, SerialStatistic};
use crate::experimental::series::{SerializedBuffer, TimeMatrix, TimeMatrixFold, TimeMatrixTick};

pub trait InspectSender {
    /// Sends a [`TimeMatrix`] to the client's inspection server.
    ///
    /// See [`inspect_time_matrix_with_metadata`].
    ///
    /// [`inspect_time_matrix_with_metadata`]: crate::experimental::serve::TimeMatrixClient::inspect_time_matrix_with_metadata
    /// [`TimeMatrix`]: crate::experimental::series::TimeMatrix
    fn inspect_time_matrix<F, P>(
        &self,
        name: impl Into<String>,
        matrix: TimeMatrix<F, P>,
    ) -> InspectedTimeMatrix<F::Sample>
    where
        TimeMatrix<F, P>: 'static + TimeMatrixFold<F::Sample> + Send,
        Metadata<F>: 'static + Send + Sync,
        F: SerialStatistic<P>,
        F::Sample: Send,
        P: InterpolationKind;

    /// Sends a [`TimeMatrix`] to the client's inspection server.
    ///
    /// This function lazily records the given [`TimeMatrix`] to Inspect. The server end
    /// periodically interpolates the matrix and records data as needed. The returned
    /// [handle][`InspectedTimeMatrix`] can be used to fold samples into the matrix.
    ///
    /// [`InspectedTimeMatrix`]: crate::experimental::serve::InspectedTimeMatrix
    /// [`TimeMatrix`]: crate::experimental::series::TimeMatrix
    fn inspect_time_matrix_with_metadata<F, P>(
        &self,
        name: impl Into<String>,
        matrix: TimeMatrix<F, P>,
        metadata: impl Into<Metadata<F>>,
    ) -> InspectedTimeMatrix<F::Sample>
    where
        TimeMatrix<F, P>: 'static + TimeMatrixFold<F::Sample> + Send,
        Metadata<F>: 'static + Send + Sync,
        F: SerialStatistic<P>,
        F::Sample: Send,
        P: InterpolationKind;

    /// Clones the client and scopes it to a child node with the given name.
    fn clone_with_child(&self, name: &str) -> Self;
}

pub struct TimeMatrixClient {
    node: InspectNode,
}

impl TimeMatrixClient {
    /// Create a new TimeMatrixClient that holds a given Inspect Node
    ///
    /// Note: If TimeMatrixClient is constructed with a weak reference to Inspect
    /// Node, then the original Node needs to be preserved for time series
    /// data shows up in Inspect. If TimeMatrixClient is constructed with the original
    /// Inspect node, then the TimeMatrixClient itself needs to be preserved.
    pub fn new(node: InspectNode) -> Self {
        Self { node }
    }

    fn inspect_and_record_with<F, P, R>(
        &self,
        name: impl Into<String>,
        matrix: TimeMatrix<F, P>,
        record: R,
    ) -> InspectedTimeMatrix<F::Sample>
    where
        TimeMatrix<F, P>: 'static + TimeMatrixFold<F::Sample> + Send,
        Metadata<F>: 'static + Send + Sync,
        F: SerialStatistic<P>,
        F::Sample: Send,
        P: InterpolationKind,
        R: 'static + Clone + Fn(&InspectNode) + Send + Sync,
    {
        let name = name.into();
        let matrix = Arc::new(Mutex::new(matrix));
        self::record_lazy_time_matrix_with(&self.node, &name, matrix.clone(), record);
        InspectedTimeMatrix::new(name, matrix)
    }
}

impl Clone for TimeMatrixClient {
    fn clone(&self) -> Self {
        TimeMatrixClient { node: self.node.clone_weak() }
    }
}

impl InspectSender for TimeMatrixClient {
    fn inspect_time_matrix<F, P>(
        &self,
        name: impl Into<String>,
        matrix: TimeMatrix<F, P>,
    ) -> InspectedTimeMatrix<F::Sample>
    where
        TimeMatrix<F, P>: 'static + TimeMatrixFold<F::Sample> + Send,
        Metadata<F>: 'static + Send + Sync,
        F: SerialStatistic<P>,
        F::Sample: Send,
        P: InterpolationKind,
    {
        self.inspect_and_record_with(name, matrix, |_node| {})
    }

    fn inspect_time_matrix_with_metadata<F, P>(
        &self,
        name: impl Into<String>,
        matrix: TimeMatrix<F, P>,
        metadata: impl Into<Metadata<F>>,
    ) -> InspectedTimeMatrix<F::Sample>
    where
        TimeMatrix<F, P>: 'static + TimeMatrixFold<F::Sample> + Send,
        Metadata<F>: 'static + Send + Sync,
        F: SerialStatistic<P>,
        F::Sample: Send,
        P: InterpolationKind,
    {
        let metadata = Arc::new(metadata.into());
        self.inspect_and_record_with(name, matrix, move |node| {
            use crate::experimental::series::metadata::Metadata;
            metadata.record_with_parent(node);
        })
    }

    fn clone_with_child(&self, name: &str) -> Self {
        Self { node: self.node.create_child(name) }
    }
}

#[derive(Derivative)]
#[derivative(Debug, Clone)]
pub struct InspectedTimeMatrix<T> {
    name: String,
    #[derivative(Debug = "ignore")]
    matrix: Arc<Mutex<dyn TimeMatrixFold<T> + Send>>,
}

impl<T> InspectedTimeMatrix<T> {
    pub(crate) fn new(
        name: impl Into<String>,
        matrix: Arc<Mutex<dyn TimeMatrixFold<T> + Send>>,
    ) -> Self {
        Self { name: name.into(), matrix }
    }

    /// Folding a sample to the time series.
    ///
    /// Note: the timestamp for the time series is generated only after acquiring a lock.
    /// If the lock is already being held by another thread/process that reads the Inspect
    /// data, the timestamp generated may fall on the time interval after.
    ///
    /// TODO(https://fxbug.dev/457421826) - Fix issue with delayed timestamp
    pub fn fold(&self, sample: T) -> Result<(), FoldError> {
        self.matrix.lock().fold(Timed::now(sample))
    }

    pub fn fold_or_log_error(&self, sample: T) {
        if let Err(error) = self.matrix.lock().fold(Timed::now(sample)) {
            warn!("failed to fold sample into time matrix \"{}\": {:?}", self.name, error);
        }
    }
}

/// Records a lazy child node in the given node that records buffers and metadata for the given
/// time matrix.
///
/// The function `f` is passed a node to record arbitrary data after the data semantic and buffers
/// of the time matrix have been recorded using that same node.
fn record_lazy_time_matrix_with<F>(
    node: &InspectNode,
    name: impl Into<String>,
    matrix: Arc<Mutex<dyn TimeMatrixTick + Send>>,
    f: F,
) where
    F: 'static + Clone + Fn(&InspectNode) + Send + Sync,
{
    let name = name.into();
    node.record_lazy_child(name, move || {
        let matrix = matrix.clone();
        let f = f.clone();
        async move {
            let inspector = Inspector::default();
            let result = matrix.lock().tick_and_get_buffers(Timestamp::now());
            inspector.root().atomic_update(|node| {
                if result.is_ok() {
                    f(node);
                }
                SerializedBuffer::write_to_inspect_or_error(result, node);
            });
            Ok(inspector)
        }
        .boxed()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{AnyBytesProperty, assert_data_tree};
    use fuchsia_async as fasync;

    use crate::experimental::series::SamplingProfile;
    use crate::experimental::series::interpolation::{ConstantSample, LastSample};
    use crate::experimental::series::metadata::BitsetMap;
    use crate::experimental::series::statistic::{Max, Union};

    #[fuchsia::test]
    fn inspected_time_matrix_folded_sample_appears_in_inspect() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(3_000_000_000));

        let inspector = Inspector::default();
        let client = TimeMatrixClient::new(inspector.root().create_child("serve_test_node"));
        let time_matrix = TimeMatrix::<Max<u64>, ConstantSample>::new(
            SamplingProfile::highly_granular(),
            ConstantSample::default(),
        );
        let inspected_matrix = client.inspect_time_matrix("time_series_1", time_matrix);

        inspected_matrix.fold(15).unwrap();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(10_000_000_000));

        assert_data_tree!(@executor exec, inspector, root: contains {
            serve_test_node: {
                time_series_1: {
                    "type": "gauge",
                    "data": vec![
                        1u8, // version number
                        3, 0, 0, 0, // created timestamp
                        10, 0, 0, 0, // last timestamp
                        1, 0, // type: simple8b RLE; subtype: unsigned
                        16, 0, // series 1: length in bytes
                        10, 0, // series 1 granularity: 10s
                        1, 0, // number of selector elements and value blocks
                        0, 0,    // head selector index
                        1,    // number of values in last block
                        0x0f, // RLE selector
                        15, 0, 0, 0, 0, 0, 1, 0, // value 15 appears 1 time
                        7, 0, // series 2: length in bytes
                        60, 0, // series 2 granularity: 60s
                        0, 0, // number of selector elements and value blocks
                        0, 0, // head selector index
                        0, // number of values in last block
                    ]
                }
            }
        });
    }

    #[fuchsia::test]
    async fn inspect_time_matrix_then_inspect_data_tree_contains_buffers() {
        let inspector = Inspector::default();
        let client = TimeMatrixClient::new(inspector.root().create_child("serve_test_node"));
        let _matrix = client
            .inspect_time_matrix("connectivity", TimeMatrix::<Union<u64>, LastSample>::default());

        assert_data_tree!(inspector, root: contains {
            serve_test_node: {
                connectivity: {
                    "type": "bitset",
                    "data": AnyBytesProperty,
                }
            }
        });
    }

    #[fuchsia::test]
    async fn inspect_time_matrix_with_metadata_then_inspect_data_tree_contains_metadata() {
        let inspector = Inspector::default();
        let client = TimeMatrixClient::new(inspector.root().create_child("serve_test_node"));
        let _matrix = client.inspect_time_matrix_with_metadata(
            "engine",
            TimeMatrix::<Union<u64>, LastSample>::default(),
            BitsetMap::from_ordered(["check", "oil", "battery", "coolant"]),
        );

        assert_data_tree!(inspector, root: contains {
            serve_test_node: {
                engine: {
                    "type": "bitset",
                    "data": AnyBytesProperty,
                    metadata: {
                        index: {
                            "0": "check",
                            "1": "oil",
                            "2": "battery",
                            "3": "coolant",
                        }
                    }
                }
            }
        });
    }

    #[fuchsia::test]
    async fn inspected_time_matrix_clone_with_child_properly_scoped() {
        let inspector = Inspector::default();
        let client = TimeMatrixClient::new(inspector.root().create_child("serve_test_node"));
        let child_client = client.clone_with_child("child");
        let _matrix = child_client
            .inspect_time_matrix("connectivity", TimeMatrix::<Union<u64>, LastSample>::default());

        assert_data_tree!(inspector, root: contains {
            serve_test_node: {
                child: {
                    connectivity: {
                        "type": "bitset",
                        "data": AnyBytesProperty,
                    }
                }
            }
        });
    }
}
