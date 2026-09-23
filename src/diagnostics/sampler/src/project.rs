// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::config::ProjectStats;
use crate::error::Error;
use diagnostics_data::InspectData;
use diagnostics_hierarchy::{
    ArrayContent, ExponentialHistogram, LinearHistogram, Property, SelectResult,
    select_from_hierarchy,
};
use fidl::endpoints::create_proxy;
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_metrics::{
    HistogramBucket, MetricEvent, MetricEventLoggerFactoryProxy, MetricEventLoggerProxy,
    MetricEventPayload, ProjectSpec,
};
use fuchsia_inspect::{ArrayProperty, NumericProperty};
use log::{error, warn};
use sampler_config::runtime::{DataSetConfig, MetricConfig, ProjectConfig};
use sampler_config::{EventCode, MetricId, MetricType};
use selectors::SelectorExt;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

// `Project` maps the contents of a project.
//
// Project **files** are de-duplicated per project ID,
// so even if there are multiple project files with the same ID,
// there is exactly one `Project`.
#[derive(Debug)]
pub struct Project<'a> {
    logger: MetricEventLoggerProxy,
    data_sets: Vec<DataSetConfig>,
    // This cache is safe because Sampler does not share project state
    // between threads. Since it does not serve a protocol and need to spawn
    // handlers, access is always sequential when a new event arrives on the
    // single SampleSinkServer stream.
    cache: HashMap<(MetricId, Vec<EventCode>), Property>,
    stats: Option<&'a ProjectStats>,
}

impl<'a> Project<'a> {
    pub async fn new(
        logger_factory: &MetricEventLoggerFactoryProxy,
        config: ProjectConfig,
        stats: Option<&'a ProjectStats>,
    ) -> Result<Project<'a>, Error<'static>> {
        let ProjectConfig { project_id, data_sets, .. } = config;
        let (logger, logger_server) = create_proxy();
        logger_factory
            .create_metric_event_logger(
                &ProjectSpec { project_id: Some(*project_id), ..Default::default() },
                logger_server,
            )
            .await??;

        Ok(Project { logger, data_sets, cache: HashMap::new(), stats })
    }

    pub async fn log(
        &mut self,
        data: &[InspectData],
        // None when there is a last gasp happening before shutdown.
        since_startup: Option<zx::MonotonicDuration>,
    ) -> Result<Vec<MetricConfig>, Error<'_>> {
        let mut upload_once_values = vec![];
        for data_set in &mut self.data_sets {
            // This protects against the same selector being used for multiple metrics
            // across project files. It relies on the fact that every metric in one project
            // file has the same interval, and that `Project` maps to a project file.
            if let Some(since_startup) = since_startup
                && since_startup.into_seconds() % data_set.poll_rate_sec != 0
            {
                continue;
            }

            let cache = &mut self.cache;
            let mut metric_events = vec![];
            for tree in data {
                for metric in &data_set.metrics {
                    let selectors_for_this_tree = tree
                        .moniker
                        .match_against_selectors(metric.selectors.iter())
                        .filter_map(|s| match s {
                            Ok(s) => Some(s),
                            Err(e) => {
                                error!(
                                    e:?;
                                    "Invalid selector. Fix config or file a bug against Sampler."
                                );
                                None
                            }
                        });
                    let Some(item) = Self::select_first_match(selectors_for_this_tree, tree) else {
                        continue;
                    };

                    match Self::convert_to_metric(cache, item, metric) {
                        Ok(Some(m)) => {
                            if let Some(stats) = &self.stats {
                                stats.events.borrow_mut().add_entry(|n| {
                                    n.record_uint("metric_id", *metric.metric_id as u64);
                                    let ec = n.create_uint_array("ec", metric.event_codes.len());
                                    for (idx, c) in metric.event_codes.iter().enumerate() {
                                        ec.set(idx, **c as u64);
                                    }
                                    n.record(ec);
                                });
                            }
                            metric_events.push(m);
                        }
                        Ok(None) => {}
                        Err(err) => warn!(err:?; "Error converting Inspect to Cobalt metric"),
                    }
                }
            }

            self.logger.log_metric_events(&metric_events).await??;
            self.stats.map(|stats| stats.cobalt_logs_sent.add(metric_events.len() as u64));

            let mut original_metrics = vec![];

            std::mem::swap(&mut original_metrics, &mut data_set.metrics);
            let (upload_once_values_for_data_set, filtered_metrics) =
                original_metrics.into_iter().partition(|metric| {
                    metric.upload_once
                        && metric_events.iter().any(|me| {
                            me.metric_id == *metric.metric_id
                                && me
                                    .event_codes
                                    .iter()
                                    .copied()
                                    .map(EventCode)
                                    .eq(metric.event_codes.iter().copied())
                        })
                });

            data_set.metrics = filtered_metrics;
            upload_once_values.extend(upload_once_values_for_data_set);
        }

        Ok(upload_once_values)
    }

    fn convert_to_metric<'b>(
        cache: &mut HashMap<(MetricId, Vec<EventCode>), Property>,
        item: SelectResult<'b, String>,
        metric: &MetricConfig,
    ) -> Result<Option<MetricEvent>, Error<'b>> {
        match metric.metric_type {
            MetricType::Occurrence => {
                let SelectResult::Properties(prop) = item else {
                    return Err(Error::InvalidSelectResult(item));
                };

                let new_value = Self::extract_occurrence(&prop[0])?;

                let cached_value = match cache.entry((metric.metric_id, metric.event_codes.clone()))
                {
                    Entry::Occupied(mut v) => {
                        let old = Self::extract_occurrence(v.get())?;
                        if old == new_value {
                            // This can happen if the same selector is used for different
                            // types of data. It's not an error, but we also shouldn't publish
                            // the data.
                            return Ok(None);
                        } else if new_value < old {
                            // We don't update the cache if the new_value is wrong. That means
                            // future samples will be compared to not-the-most-recent value.
                            return Err(Error::OccurrenceDecreased {
                                prior: old,
                                current: new_value,
                            });
                        }

                        v.insert(prop[0].clone().into_owned());
                        Some(old)
                    }
                    Entry::Vacant(v) => {
                        v.insert(prop[0].clone().into_owned());
                        None
                    }
                };

                Ok(Some(MetricEvent {
                    metric_id: *metric.metric_id,
                    payload: MetricEventPayload::Count(
                        cached_value.map(|c| new_value - c).unwrap_or(new_value),
                    ),
                    event_codes: metric.event_codes.iter().map(|ec| **ec).collect(),
                }))
            }
            MetricType::Integer => {
                let SelectResult::Properties(prop) = item else {
                    return Err(Error::InvalidSelectResult(item));
                };

                let new_value = Self::extract_integer(&prop[0])?;

                Ok(Some(MetricEvent {
                    metric_id: *metric.metric_id,
                    payload: MetricEventPayload::IntegerValue(new_value),
                    event_codes: metric.event_codes.iter().map(|ec| **ec).collect(),
                }))
            }
            MetricType::String => {
                let SelectResult::Properties(prop) = item else {
                    return Err(Error::InvalidSelectResult(item));
                };

                let new_value = match &*prop[0] {
                    Property::String(_, s) => s.clone(),
                    actual => {
                        return Err(Error::InvalidPropertyExpectedString(actual.clone()));
                    }
                };

                Ok(Some(MetricEvent {
                    metric_id: *metric.metric_id,
                    payload: MetricEventPayload::StringValue(new_value),
                    event_codes: metric.event_codes.iter().map(|ec| **ec).collect(),
                }))
            }
            MetricType::IntHistogram => {
                let SelectResult::Properties(prop) = item else {
                    return Err(Error::InvalidSelectResult(item));
                };

                let Some(payload) =
                    (match cache.entry((metric.metric_id, metric.event_codes.clone())) {
                        Entry::Occupied(mut v) => {
                            let payload = Self::process_int_histogram(&prop[0], Some(v.get()))?;
                            // only update the cache if the histogram is valid
                            if payload.is_some() {
                                v.insert(prop[0].clone().into_owned());
                            }
                            payload
                        }
                        Entry::Vacant(v) => {
                            v.insert(prop[0].clone().into_owned());
                            Self::process_int_histogram(&prop[0], None)?
                        }
                    })
                else {
                    return Ok(None);
                };

                Ok(Some(MetricEvent {
                    metric_id: *metric.metric_id,
                    payload,
                    event_codes: metric.event_codes.iter().map(|ec| **ec).collect(),
                }))
            }
        }
    }

    // Returns Ok(None) when, after compaction/conversion to cobalt histogram, there
    // is no longer a diff to send. The semantics of fuchsia.diagnostics.Sample should
    // prevent this from happening, but better to just handle that path here because
    // it's not demonstrable in this code that those invariants hold.
    fn process_int_histogram(
        new_sample: &Property,
        prev_sample_opt: Option<&Property>,
    ) -> Result<Option<MetricEventPayload>, Error<'static>> {
        let diff = match prev_sample_opt {
            None => Self::convert_inspect_histogram_to_cobalt_histogram(new_sample)?,
            Some(prev_sample) => Self::compute_histogram_diff(new_sample, prev_sample)?,
        };

        let non_empty_diff: Vec<HistogramBucket> =
            diff.into_iter().filter(|v| v.count != 0).collect();
        if !non_empty_diff.is_empty() {
            Ok(Some(MetricEventPayload::Histogram(non_empty_diff)))
        } else {
            Ok(None)
        }
    }

    fn histogram_metadata_matches(one: &Property, two: &Property) -> bool {
        match (one, two) {
            (
                Property::IntArray(
                    _,
                    ArrayContent::LinearHistogram(LinearHistogram {
                        floor: floor1,
                        step: step1,
                        ..
                    }),
                ),
                Property::IntArray(
                    _,
                    ArrayContent::LinearHistogram(LinearHistogram {
                        floor: floor2,
                        step: step2,
                        ..
                    }),
                ),
            ) => floor1 == floor2 && step1 == step2,
            (
                Property::IntArray(
                    _,
                    ArrayContent::ExponentialHistogram(ExponentialHistogram {
                        floor: floor1,
                        initial_step: initial_step1,
                        step_multiplier: step_multiplier1,
                        ..
                    }),
                ),
                Property::IntArray(
                    _,
                    ArrayContent::ExponentialHistogram(ExponentialHistogram {
                        floor: floor2,
                        initial_step: initial_step2,
                        step_multiplier: step_multiplier2,
                        ..
                    }),
                ),
            ) => {
                floor1 == floor2
                    && initial_step1 == initial_step2
                    && step_multiplier1 == step_multiplier2
            }
            (
                Property::UintArray(
                    _,
                    ArrayContent::LinearHistogram(LinearHistogram {
                        floor: floor1,
                        step: step1,
                        ..
                    }),
                ),
                Property::UintArray(
                    _,
                    ArrayContent::LinearHistogram(LinearHistogram {
                        floor: floor2,
                        step: step2,
                        ..
                    }),
                ),
            ) => floor1 == floor2 && step1 == step2,
            (
                Property::UintArray(
                    _,
                    ArrayContent::ExponentialHistogram(ExponentialHistogram {
                        floor: floor1,
                        initial_step: initial_step1,
                        step_multiplier: step_multiplier1,
                        ..
                    }),
                ),
                Property::UintArray(
                    _,
                    ArrayContent::ExponentialHistogram(ExponentialHistogram {
                        floor: floor2,
                        initial_step: initial_step2,
                        step_multiplier: step_multiplier2,
                        ..
                    }),
                ),
            ) => {
                floor1 == floor2
                    && initial_step1 == initial_step2
                    && step_multiplier1 == step_multiplier2
            }
            _ => false,
        }
    }

    fn compute_histogram_diff(
        new_sample: &Property,
        old_sample: &Property,
    ) -> Result<Vec<HistogramBucket>, Error<'static>> {
        if !Self::histogram_metadata_matches(new_sample, old_sample) {
            return Err(Error::PropertyTypeChangedBetweenSamples);
        }

        let new_histogram_buckets =
            Self::convert_inspect_histogram_to_cobalt_histogram(new_sample)?;
        let old_histogram_buckets =
            Self::convert_inspect_histogram_to_cobalt_histogram(old_sample)?;

        if old_histogram_buckets.len() != new_histogram_buckets.len() {
            return Err(Error::NumberOfHistogramBucketsChanged);
        }

        new_histogram_buckets
            .iter()
            .zip(old_histogram_buckets)
            .map(|(new_bucket, old_bucket)| {
                if new_bucket.count < old_bucket.count {
                    return Err(Error::HistogramBucketCountDecreased {
                        index: new_bucket.index,
                        old_count: old_bucket.count,
                        new_count: new_bucket.count,
                    });
                }
                Ok(HistogramBucket {
                    count: new_bucket.count - old_bucket.count,
                    index: new_bucket.index,
                })
            })
            .collect::<Result<Vec<HistogramBucket>, Error<'_>>>()
    }

    fn build_cobalt_histogram(counts: impl Iterator<Item = u64>) -> Vec<HistogramBucket> {
        counts
            .enumerate()
            .map(|(index, count)| HistogramBucket { index: index as u32, count })
            .collect()
    }

    fn build_sparse_cobalt_histogram(
        counts: impl Iterator<Item = u64>,
        indexes: &[usize],
        size: usize,
    ) -> Vec<HistogramBucket> {
        let mut histogram = Vec::from_iter(
            (0..size).map(|index| HistogramBucket { index: index as u32, count: 0 }),
        );
        for (index, count) in indexes.iter().zip(counts) {
            histogram[*index].count = count;
        }
        histogram
    }

    fn convert_inspect_histogram_to_cobalt_histogram(
        inspect_histogram: &Property,
    ) -> Result<Vec<HistogramBucket>, Error<'static>> {
        let sanitize_size = |size: usize| -> Result<(), Error<'_>> {
            if size > u32::MAX as usize {
                return Err(Error::IndexTooLargeForU32);
            }
            Ok(())
        };

        let sanitize_indexes = |indexes: &[usize], size: usize| -> Result<(), Error<'_>> {
            for index in indexes.iter() {
                if *index >= size {
                    return Err(Error::HistogramIndexOutOfBounds);
                }
            }
            Ok(())
        };

        let sanitize_counts = |counts: &[i64]| -> Result<(), Error<'_>> {
            for count in counts.iter() {
                if *count < 0 {
                    return Err(Error::HistogramHasNegativeCount);
                }
            }
            Ok(())
        };

        let histogram = match inspect_histogram {
            Property::IntArray(
                _,
                ArrayContent::LinearHistogram(LinearHistogram { counts, indexes, size, .. })
                | ArrayContent::ExponentialHistogram(ExponentialHistogram {
                    counts,
                    indexes,
                    size,
                    ..
                }),
            ) => {
                sanitize_size(*size)?;
                sanitize_counts(counts)?;
                match (indexes, counts) {
                    (None, counts) => {
                        Self::build_cobalt_histogram(counts.iter().map(|c| *c as u64))
                    }
                    (Some(indexes), counts) => {
                        sanitize_indexes(indexes, *size)?;
                        Self::build_sparse_cobalt_histogram(
                            counts.iter().map(|c| *c as u64),
                            indexes,
                            *size,
                        )
                    }
                }
            }
            Property::UintArray(
                _,
                ArrayContent::LinearHistogram(LinearHistogram { counts, indexes, size, .. })
                | ArrayContent::ExponentialHistogram(ExponentialHistogram {
                    counts,
                    indexes,
                    size,
                    ..
                }),
            ) => {
                sanitize_size(*size)?;
                match (indexes, counts) {
                    (None, counts) => Self::build_cobalt_histogram(counts.iter().copied()),
                    (Some(indexes), counts) => {
                        sanitize_indexes(indexes, *size)?;
                        Self::build_sparse_cobalt_histogram(counts.iter().copied(), indexes, *size)
                    }
                }
            }
            Property::DoubleArray(
                _,
                ArrayContent::LinearHistogram(_) | ArrayContent::ExponentialHistogram(_),
            ) => return Err(Error::DoubleHistogramsUnsupported),
            _ => {
                return Err(Error::UnexpectedPropertyTypeForHistogram);
            }
        };
        Ok(histogram)
    }

    fn extract_occurrence(property: &Property<String>) -> Result<u64, Error<'static>> {
        match property {
            Property::Int(_, i) => Ok((*i).try_into()?),
            Property::Uint(_, u) => Ok(*u),
            actual => Err(Error::InvalidPropertyExpectedIntOrUint(actual.clone())),
        }
    }

    fn extract_integer(property: &Property<String>) -> Result<i64, Error<'static>> {
        match property {
            Property::Int(_, i) => Ok(*i),
            Property::Uint(_, u) => Ok(*u as i64),
            actual => Err(Error::InvalidPropertyExpectedIntOrUint(actual.clone())),
        }
    }

    fn select_first_match<'b>(
        selectors: impl Iterator<Item = &'b fdiagnostics::Selector>,
        tree: &'b InspectData,
    ) -> Option<SelectResult<'b, String>> {
        for s in selectors {
            let Some(payload) = tree.payload.as_ref() else {
                continue;
            };
            let hit = match select_from_hierarchy(payload, s) {
                Ok(p) => p,
                Err(e) => {
                    error!(
                        e:?;
                        "Invalid selector. Fix config or file a bug against Sampler."
                    );
                    continue;
                }
            };

            // This is checking whether any actual data was matched by the selector.
            // An error is different than not finding any data where you thought it might be.
            match &hit {
                SelectResult::Properties(props) if !props.is_empty() => {}
                SelectResult::Nodes(nodes) if !nodes.is_empty() => {}
                SelectResult::Nodes(_) | SelectResult::Properties(_) => continue,
            }

            return Some(hit);
        }

        None
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;
    use diagnostics_data::{DiagnosticsHierarchy, InspectDataBuilder, Timestamp};
    use diagnostics_hierarchy::{
        ArrayContent, ExponentialHistogram, LinearHistogram, Property, SelectResult, hierarchy,
    };
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_diagnostics::Selector;
    use fidl_fuchsia_metrics::{
        HistogramBucket, MetricEventLoggerMarker, MetricEventLoggerRequest,
        MetricEventLoggerRequestStream, MetricEventPayload,
    };
    use fuchsia_async::Scope;
    use fuchsia_sync::Mutex;
    use futures::StreamExt;
    use sampler_config::runtime::MetricConfig;
    use sampler_config::{MetricId, MetricType};
    use selectors::parse_verbose;
    use std::borrow::Cow;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn create_metric_config(metric_id: u32, metric_type: MetricType) -> MetricConfig {
        MetricConfig {
            metric_id: MetricId(metric_id),
            metric_type,
            event_codes: vec![],
            selectors: vec![],
            upload_once: false,
        }
    }

    // needs to be async for create_proxy (or manually create executor)
    async fn create_empty_project() -> Project<'static> {
        let (logger, _) = fidl::endpoints::create_proxy::<MetricEventLoggerMarker>();
        Project { logger, data_sets: vec![], cache: HashMap::new(), stats: None }
    }

    fn test_data(timestamp_nanos: i64, hierarchy: DiagnosticsHierarchy) -> Vec<InspectData> {
        vec![
            InspectDataBuilder::new(
                "moniker/for/test".try_into().unwrap(),
                "fuchsia-pkg://test",
                Timestamp::from_nanos(timestamp_nanos),
            )
            .with_hierarchy(hierarchy)
            .build(),
        ]
    }

    fn histogram_hierarchy(counts: Vec<i64>) -> DiagnosticsHierarchy {
        DiagnosticsHierarchy::new(
            "root",
            vec![Property::IntArray(
                "foo".to_string(),
                ArrayContent::LinearHistogram(LinearHistogram {
                    floor: 0i64,
                    step: 1,
                    counts,
                    indexes: None,
                    size: 2,
                }),
            )],
            vec![],
        )
    }

    async fn wait_for_data(data_sink: &Arc<Mutex<Vec<MetricEvent>>>) -> Vec<MetricEvent> {
        while data_sink.lock().is_empty() {
            fuchsia_async::Timer::new(zx::MonotonicDuration::from_seconds(1)).await;
        }
        data_sink.lock().clone()
    }

    async fn serve_mock_cobalt_logger(
        data_sink: Arc<Mutex<Vec<MetricEvent>>>,
        mut stream: MetricEventLoggerRequestStream,
    ) {
        while let Some(Ok(data)) = stream.next().await {
            match data {
                MetricEventLoggerRequest::LogMetricEvents { mut events, responder } => {
                    data_sink.lock().append(&mut events);
                    responder.send(Ok(())).unwrap();
                }
                _ => unimplemented!(),
            }
        }
    }

    #[fuchsia::test]
    async fn test_log_histogram() {
        let (logger, logger_server) = create_proxy_and_stream::<MetricEventLoggerMarker>();
        let mut project = Project {
            logger,
            data_sets: vec![DataSetConfig {
                metrics: vec![MetricConfig {
                    selectors: vec![parse_verbose("moniker/for/test:root:foo").unwrap()],
                    metric_id: MetricId(0),
                    metric_type: MetricType::IntHistogram,
                    event_codes: vec![EventCode(1), EventCode(2), EventCode(3)],
                    upload_once: false,
                }],
                poll_rate_sec: 1,
            }],
            cache: HashMap::new(),
            stats: None,
        };

        let data_sink = Arc::new(Mutex::new(vec![]));
        let scope = Scope::new();
        scope.spawn(serve_mock_cobalt_logger(data_sink.clone(), logger_server));

        {
            let data = test_data(0, histogram_hierarchy(vec![1, 0]));
            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(0))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::Histogram(vec![HistogramBucket {
                    index: 0,
                    count: 1,
                }]),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(1, histogram_hierarchy(vec![2, 0]));
            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::Histogram(vec![HistogramBucket {
                    index: 0,
                    count: 1,
                }]),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(1, histogram_hierarchy(vec![2, 1]));
            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::Histogram(vec![HistogramBucket {
                    index: 1,
                    count: 1,
                }]),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(1, histogram_hierarchy(vec![3, 2]));
            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::Histogram(vec![
                    HistogramBucket { index: 0, count: 1 },
                    HistogramBucket { index: 1, count: 1 },
                ]),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            // decrease from the previous value which is in the cache
            let prop = Property::IntArray(
                "foo".to_string(),
                ArrayContent::LinearHistogram(LinearHistogram {
                    floor: 0i64,
                    step: 1,
                    counts: vec![3, 1],
                    indexes: None,
                    size: 2,
                }),
            );
            let config = &project.data_sets[0].metrics[0];
            let actual = Project::convert_to_metric(
                &mut project.cache,
                SelectResult::Properties(vec![Cow::Owned(prop)]),
                config,
            );
            assert_matches!(
                actual,
                Err(Error::HistogramBucketCountDecreased { index: 1, old_count: 2, new_count: 1 })
            );
        }
    }

    #[fuchsia::test]
    async fn test_log_string() {
        let (logger, logger_server) = create_proxy_and_stream::<MetricEventLoggerMarker>();
        let mut project = Project {
            logger,
            data_sets: vec![DataSetConfig {
                metrics: vec![MetricConfig {
                    selectors: vec![parse_verbose("moniker/for/test:root:foo").unwrap()],
                    metric_id: MetricId(0),
                    metric_type: MetricType::String,
                    event_codes: vec![EventCode(1), EventCode(2), EventCode(3)],
                    upload_once: false,
                }],
                poll_rate_sec: 1,
            }],
            cache: HashMap::new(),
            stats: None,
        };

        let data_sink = Arc::new(Mutex::new(vec![]));
        let scope = Scope::new();
        scope.spawn(serve_mock_cobalt_logger(data_sink.clone(), logger_server));

        {
            let data = test_data(
                0,
                hierarchy! {
                    root: {
                        foo: "hello",
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(0))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::StringValue("hello".to_string()),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(
                1,
                hierarchy! {
                    root: {
                        foo: "world",
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::StringValue("world".to_string()),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(
                1,
                hierarchy! {
                    root: {
                        foo: "world",
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::StringValue("world".to_string()),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }
    }

    #[fuchsia::test]
    async fn test_log_integer() {
        let (logger, logger_server) = create_proxy_and_stream::<MetricEventLoggerMarker>();
        let mut project = Project {
            logger,
            data_sets: vec![DataSetConfig {
                metrics: vec![MetricConfig {
                    selectors: vec![parse_verbose("moniker/for/test:root:foo").unwrap()],
                    metric_id: MetricId(0),
                    metric_type: MetricType::Integer,
                    event_codes: vec![EventCode(1), EventCode(2), EventCode(3)],
                    upload_once: false,
                }],
                poll_rate_sec: 1,
            }],
            cache: HashMap::new(),
            stats: None,
        };

        let data_sink = Arc::new(Mutex::new(vec![]));
        let scope = Scope::new();
        scope.spawn(serve_mock_cobalt_logger(data_sink.clone(), logger_server));

        {
            let data = test_data(
                0,
                hierarchy! {
                    root: {
                        foo: 1i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(0))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::IntegerValue(1),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(
                1,
                hierarchy! {
                    root: {
                        foo: 10i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::IntegerValue(10),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(
                1,
                hierarchy! {
                    root: {
                        foo: 10i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::IntegerValue(10),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(
                1,
                hierarchy! {
                    root: {
                        foo: 9i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::IntegerValue(9),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(
                1,
                hierarchy! {
                    root: {
                        foo: -9i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::IntegerValue(-9),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }
    }

    #[fuchsia::test]
    async fn test_log_occurrence() {
        let (logger, logger_server) = create_proxy_and_stream::<MetricEventLoggerMarker>();
        let mut project = Project {
            logger,
            data_sets: vec![DataSetConfig {
                metrics: vec![MetricConfig {
                    selectors: vec![parse_verbose("moniker/for/test:root:foo").unwrap()],
                    metric_id: MetricId(0),
                    metric_type: MetricType::Occurrence,
                    event_codes: vec![EventCode(1), EventCode(2), EventCode(3)],
                    upload_once: false,
                }],
                poll_rate_sec: 1,
            }],
            cache: HashMap::new(),
            stats: None,
        };

        let data_sink = Arc::new(Mutex::new(vec![]));
        let scope = Scope::new();
        scope.spawn(serve_mock_cobalt_logger(data_sink.clone(), logger_server));

        {
            let data = test_data(
                0,
                hierarchy! {
                    root: {
                        foo: 1i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(0))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::Count(1),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(
                1,
                hierarchy! {
                    root: {
                        foo: 10i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::Count(9),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(
                1,
                hierarchy! {
                    root: {
                        foo: 10i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            fuchsia_async::Timer::new(zx::MonotonicDuration::from_seconds(1)).await;

            assert!(data_sink.lock().is_empty());
            assert!(upload_once.is_empty());
        }

        {
            // decrease from the previous value of 10 which is in the cache
            let int_prop = Property::Int("value".to_string(), 9);
            let config = &project.data_sets[0].metrics[0];
            let actual = Project::convert_to_metric(
                &mut project.cache,
                SelectResult::Properties(vec![Cow::Owned(int_prop)]),
                config,
            );
            assert_matches!(actual, Err(Error::OccurrenceDecreased { prior: 10, current: 9 }));
        }
    }

    #[fuchsia::test]
    fn extract_occurrence_test() {
        let int_prop = Property::Int("value".to_string(), 10);
        assert_eq!(Project::extract_occurrence(&int_prop).unwrap(), 10);

        let uint_prop = Property::Uint("value".to_string(), 20);
        assert_eq!(Project::extract_occurrence(&uint_prop).unwrap(), 20);

        let string_prop = Property::String("value".to_string(), "hello".to_string());
        let actual = Project::extract_occurrence(&string_prop).unwrap_err();
        assert_matches!(actual, Error::InvalidPropertyExpectedIntOrUint(s) if s == string_prop);
    }

    #[fuchsia::test]
    fn extract_integer_test() {
        let int_prop = Property::Int("value".to_string(), -10);
        assert_eq!(Project::extract_integer(&int_prop).unwrap(), -10);

        let uint_prop = Property::Uint("value".to_string(), 20);
        assert_eq!(Project::extract_integer(&uint_prop).unwrap(), 20);

        let string_prop = Property::String("value".to_string(), "hello".to_string());
        let actual = Project::extract_integer(&string_prop).unwrap_err();
        assert_matches!(actual, Error::InvalidPropertyExpectedIntOrUint(s) if s == string_prop);
    }

    #[fuchsia::test]
    fn convert_inspect_linear_int_histogram_to_cobalt_histogram_dense() {
        let inspect_histogram = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0,
                step: 1,
                counts: vec![1, 2, 3],
                indexes: None,
                size: 3,
            }),
        );

        let cobalt_histogram =
            Project::convert_inspect_histogram_to_cobalt_histogram(&inspect_histogram).unwrap();

        assert_eq!(
            cobalt_histogram,
            vec![
                HistogramBucket { index: 0, count: 1 },
                HistogramBucket { index: 1, count: 2 },
                HistogramBucket { index: 2, count: 3 },
            ]
        );
    }

    #[fuchsia::test]
    fn convert_inspect_linear_int_histogram_to_cobalt_histogram_sparse() {
        let inspect_histogram = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0,
                step: 1,
                counts: vec![10, 20],
                indexes: Some(vec![1, 3]),
                size: 5,
            }),
        );

        let cobalt_histogram =
            Project::convert_inspect_histogram_to_cobalt_histogram(&inspect_histogram).unwrap();

        assert_eq!(
            cobalt_histogram,
            vec![
                HistogramBucket { index: 0, count: 0 },
                HistogramBucket { index: 1, count: 10 },
                HistogramBucket { index: 2, count: 0 },
                HistogramBucket { index: 3, count: 20 },
                HistogramBucket { index: 4, count: 0 },
            ]
        );
    }

    #[fuchsia::test]
    fn convert_inspect_histogram_errors() {
        // Double histograms are not supported.
        let double_histogram = Property::DoubleArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0.0,
                step: 1.0,
                counts: vec![],
                indexes: None,
                size: 0,
            }),
        );
        let actual =
            Project::convert_inspect_histogram_to_cobalt_histogram(&double_histogram).unwrap_err();
        assert_matches!(actual, Error::DoubleHistogramsUnsupported);

        // Wrong property type
        let not_a_histogram = Property::String("value".to_string(), "hello".to_string());
        let actual =
            Project::convert_inspect_histogram_to_cobalt_histogram(&not_a_histogram).unwrap_err();
        assert_matches!(actual, Error::UnexpectedPropertyTypeForHistogram);

        // Negative count in int histogram
        let negative_count_histogram = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                counts: vec![-1],
                floor: 0,
                step: 1,
                indexes: None,
                size: 1,
            }),
        );
        let actual =
            Project::convert_inspect_histogram_to_cobalt_histogram(&negative_count_histogram)
                .unwrap_err();
        assert_matches!(actual, Error::HistogramHasNegativeCount);

        // Index out of bounds
        let out_of_bounds_histogram = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                counts: vec![1],
                indexes: Some(vec![5]),
                size: 3,
                floor: 0,
                step: 1,
            }),
        );
        let actual =
            Project::convert_inspect_histogram_to_cobalt_histogram(&out_of_bounds_histogram)
                .unwrap_err();
        assert_matches!(actual, Error::HistogramIndexOutOfBounds);

        // Size too large for u32 index
        let large_size_histogram = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                size: u32::MAX as usize + 1,
                floor: 0,
                step: 1,
                counts: vec![],
                indexes: None,
            }),
        );

        let actual = Project::convert_inspect_histogram_to_cobalt_histogram(&large_size_histogram)
            .unwrap_err();
        assert_matches!(actual, Error::IndexTooLargeForU32);
    }

    #[fuchsia::test]
    fn compute_histogram_diff_test_metadata() {
        let old_sample = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0,
                step: 1,
                counts: vec![1, 2, 3],
                indexes: None,
                size: 3,
            }),
        );

        // Metadata matches, so it should error out due to the bug where it returns an
        // error on matching metadata instead of mismatching metadata.
        let new_sample_matching_metadata = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0,
                step: 1,
                counts: vec![2, 4, 6],
                indexes: None,
                size: 3,
            }),
        );
        assert_eq!(
            Project::compute_histogram_diff(&new_sample_matching_metadata, &old_sample).unwrap(),
            vec![
                HistogramBucket { index: 0, count: 1 },
                HistogramBucket { index: 1, count: 2 },
                HistogramBucket { index: 2, count: 3 },
            ]
        );

        // Metadata does not match, so it should proceed without error.
        let new_sample_mismatching_metadata = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 10, // different
                step: 1,
                counts: vec![2, 4, 6],
                indexes: None,
                size: 3,
            }),
        );
        let actual = Project::compute_histogram_diff(&new_sample_mismatching_metadata, &old_sample)
            .unwrap_err();
        assert_matches!(actual, Error::PropertyTypeChangedBetweenSamples);
    }

    #[fuchsia::test]
    fn compute_histogram_diff_decreasing_count_is_error() {
        let old_sample = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0,
                step: 1,
                counts: vec![1, 2, 3],
                indexes: None,
                size: 3,
            }),
        );

        let new_sample = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0,
                step: 1,
                counts: vec![2, 1, 6], // second bucket decreased
                indexes: None,
                size: 3,
            }),
        );

        let result = Project::compute_histogram_diff(&new_sample, &old_sample).unwrap_err();
        assert_matches!(
            result,
            Error::HistogramBucketCountDecreased { index: 1, old_count: 2, new_count: 1 }
        );
    }

    #[fuchsia::test]
    fn compute_histogram_diff_bucket_count_changed_is_error() {
        let old_sample = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0,
                step: 1,
                counts: vec![1, 2, 3],
                indexes: None,
                size: 3,
            }),
        );

        let new_sample = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0,
                step: 1,
                counts: vec![2, 4, 6, 8],
                indexes: None,
                size: 4,
            }),
        );

        let result = Project::compute_histogram_diff(&new_sample, &old_sample).unwrap_err();
        assert_matches!(result, Error::NumberOfHistogramBucketsChanged);
    }

    #[fuchsia::test]
    fn select_first_match_test() {
        let hierarchy = hierarchy! {
            root: {
                child1: {
                    value: 10i64,
                },
                child2: {
                    value: 20u64,
                }
            }
        };

        let inspect_data = InspectDataBuilder::new(
            "moniker/for/test".try_into().unwrap(),
            "fuchsia-pkg://test",
            Timestamp::from_nanos(0i64),
        )
        .with_hierarchy(hierarchy)
        .build();

        let selector1: Selector = parse_verbose("moniker/for/test:root/child1:value").unwrap();
        let selector2: Selector = parse_verbose("moniker/for/test:root/child2:value").unwrap();
        let selector_no_match: Selector =
            parse_verbose("moniker/for/test:root/child3:value").unwrap();

        // First selector matches
        let selectors = vec![&selector1, &selector2];
        let result = Project::select_first_match(selectors.into_iter(), &inspect_data).unwrap();
        match result {
            SelectResult::Properties(props) => {
                assert_eq!(props.len(), 1);
                assert_eq!(&*props[0], &Property::Int("value".to_string(), 10));
            }
            _ => panic!("Expected properties"),
        }

        // Second selector matches
        let selectors = vec![&selector_no_match, &selector2];
        let result = Project::select_first_match(selectors.into_iter(), &inspect_data).unwrap();
        match result {
            SelectResult::Properties(props) => {
                assert_eq!(props.len(), 1);
                assert_eq!(&*props[0], &Property::Uint("value".to_string(), 20));
            }
            _ => panic!("Expected properties"),
        }

        // No selector matches
        let selectors = vec![&selector_no_match];
        let result = Project::select_first_match(selectors.into_iter(), &inspect_data);
        assert!(result.is_none());
    }

    #[fuchsia::test]
    async fn convert_to_metric_occurrence() {
        let mut project = create_empty_project().await;
        let metric_config = create_metric_config(1, MetricType::Occurrence);
        let prop = Property::Int("value".to_string(), 10);
        let item = SelectResult::Properties(vec![Cow::Borrowed(&prop)]);

        // First time, should be the full value.
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        assert_eq!(metric_event.payload, MetricEventPayload::Count(10));

        // Second time, with a new value, should be the diff.
        let prop = Property::Int("value".to_string(), 15);
        let item = SelectResult::Properties(vec![Cow::Borrowed(&prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        assert_eq!(metric_event.payload, MetricEventPayload::Count(5));

        // Third time, with same value, should be None.
        let item = SelectResult::Properties(vec![Cow::Borrowed(&prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap();
        assert!(metric_event.is_none());
    }

    #[fuchsia::test]
    async fn convert_to_metric_integer() {
        let mut project = create_empty_project().await;
        let metric_config = create_metric_config(1, MetricType::Integer);
        let prop = Property::Int("value".to_string(), -50);
        let item = SelectResult::Properties(vec![Cow::Borrowed(&prop)]);

        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        assert_eq!(metric_event.payload, MetricEventPayload::IntegerValue(-50));

        // second time should get the same value
        let item = SelectResult::Properties(vec![Cow::Borrowed(&prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        assert_eq!(metric_event.payload, MetricEventPayload::IntegerValue(-50));

        // using a different value should work with no relation to the previous value
        let prop = Property::Int("value".to_string(), 50);
        let item = SelectResult::Properties(vec![Cow::Borrowed(&prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        assert_eq!(metric_event.payload, MetricEventPayload::IntegerValue(50));
    }

    #[fuchsia::test]
    async fn convert_to_metric_string() {
        let mut project = create_empty_project().await;
        let metric_config = create_metric_config(1, MetricType::String);
        let prop = Property::String("value".to_string(), "test-string".to_string());
        let item = SelectResult::Properties(vec![Cow::Borrowed(&prop)]);

        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        assert_eq!(
            metric_event.payload,
            MetricEventPayload::StringValue("test-string".to_string())
        );

        let item = SelectResult::Properties(vec![Cow::Borrowed(&prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        assert_eq!(
            metric_event.payload,
            MetricEventPayload::StringValue("test-string".to_string())
        );

        let prop = Property::String("value".to_string(), "changed-string".to_string());
        let item = SelectResult::Properties(vec![Cow::Borrowed(&prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        assert_eq!(
            metric_event.payload,
            MetricEventPayload::StringValue("changed-string".to_string())
        );
    }

    #[fuchsia::test]
    async fn convert_to_metric_int_histogram() {
        let mut project = create_empty_project().await;
        let metric_config = create_metric_config(1, MetricType::IntHistogram);
        let hist_prop = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0,
                step: 1,
                counts: vec![1, 2, 3],
                indexes: None,
                size: 3,
            }),
        );
        let item = SelectResult::Properties(vec![Cow::Borrowed(&hist_prop)]);

        // First time, full histogram.
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        match metric_event.payload {
            MetricEventPayload::Histogram(buckets) => {
                assert_eq!(
                    buckets,
                    vec![
                        HistogramBucket { index: 0, count: 1 },
                        HistogramBucket { index: 1, count: 2 },
                        HistogramBucket { index: 2, count: 3 },
                    ]
                );
            }
            _ => panic!("Wrong payload type"),
        }

        // Second time, diff histogram.
        let updated_hist_prop = Property::IntArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0,
                step: 1,
                counts: vec![2, 4, 6],
                indexes: None,
                size: 3,
            }),
        );
        let item = SelectResult::Properties(vec![Cow::Borrowed(&updated_hist_prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        match metric_event.payload {
            MetricEventPayload::Histogram(buckets) => {
                assert_eq!(
                    buckets,
                    vec![
                        HistogramBucket { index: 0, count: 1 },
                        HistogramBucket { index: 1, count: 2 },
                        HistogramBucket { index: 2, count: 3 },
                    ]
                );
            }
            _ => panic!("Wrong payload type"),
        }

        // Third time, same value, should be None.
        let item = SelectResult::Properties(vec![Cow::Borrowed(&updated_hist_prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap();
        assert!(metric_event.is_none());
    }

    #[fuchsia::test]
    async fn convert_to_metric_int_exponential_histogram() {
        let mut project = create_empty_project().await;
        let metric_config = create_metric_config(1, MetricType::IntHistogram);
        let hist_prop = Property::IntArray(
            "hist".to_string(),
            ArrayContent::ExponentialHistogram(ExponentialHistogram {
                floor: 0,
                initial_step: 1,
                step_multiplier: 2,
                counts: vec![1, 2, 3],
                indexes: None,
                size: 3,
            }),
        );
        let item = SelectResult::Properties(vec![Cow::Borrowed(&hist_prop)]);

        // First time, full histogram.
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        match metric_event.payload {
            MetricEventPayload::Histogram(buckets) => {
                assert_eq!(
                    buckets,
                    vec![
                        HistogramBucket { index: 0, count: 1 },
                        HistogramBucket { index: 1, count: 2 },
                        HistogramBucket { index: 2, count: 3 },
                    ]
                );
            }
            _ => panic!("Wrong payload type"),
        }

        // Second time, diff histogram.
        let updated_hist_prop = Property::IntArray(
            "hist".to_string(),
            ArrayContent::ExponentialHistogram(ExponentialHistogram {
                floor: 0,
                initial_step: 1,
                step_multiplier: 2,
                counts: vec![2, 4, 6],
                indexes: None,
                size: 3,
            }),
        );
        let item = SelectResult::Properties(vec![Cow::Borrowed(&updated_hist_prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        match metric_event.payload {
            MetricEventPayload::Histogram(buckets) => {
                assert_eq!(
                    buckets,
                    vec![
                        HistogramBucket { index: 0, count: 1 },
                        HistogramBucket { index: 1, count: 2 },
                        HistogramBucket { index: 2, count: 3 },
                    ]
                );
            }
            _ => panic!("Wrong payload type"),
        }

        // Third time, same value, should be None.
        let item = SelectResult::Properties(vec![Cow::Borrowed(&updated_hist_prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap();
        assert!(metric_event.is_none());
    }

    #[fuchsia::test]
    async fn convert_to_metric_uint_linear_histogram() {
        let mut project = create_empty_project().await;
        let metric_config = create_metric_config(1, MetricType::IntHistogram);
        let hist_prop = Property::UintArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0,
                step: 1,
                counts: vec![1, 2, 3],
                indexes: None,
                size: 3,
            }),
        );
        let item = SelectResult::Properties(vec![Cow::Borrowed(&hist_prop)]);

        // First time, full histogram.
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        match metric_event.payload {
            MetricEventPayload::Histogram(buckets) => {
                assert_eq!(
                    buckets,
                    vec![
                        HistogramBucket { index: 0, count: 1 },
                        HistogramBucket { index: 1, count: 2 },
                        HistogramBucket { index: 2, count: 3 },
                    ]
                );
            }
            _ => panic!("Wrong payload type"),
        }

        // Second time, diff histogram.
        let updated_hist_prop = Property::UintArray(
            "hist".to_string(),
            ArrayContent::LinearHistogram(LinearHistogram {
                floor: 0,
                step: 1,
                counts: vec![2, 4, 6],
                indexes: None,
                size: 3,
            }),
        );
        let item = SelectResult::Properties(vec![Cow::Borrowed(&updated_hist_prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        match metric_event.payload {
            MetricEventPayload::Histogram(buckets) => {
                assert_eq!(
                    buckets,
                    vec![
                        HistogramBucket { index: 0, count: 1 },
                        HistogramBucket { index: 1, count: 2 },
                        HistogramBucket { index: 2, count: 3 },
                    ]
                );
            }
            _ => panic!("Wrong payload type"),
        }

        // Third time, same value, should be None.
        let item = SelectResult::Properties(vec![Cow::Borrowed(&updated_hist_prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap();
        assert!(metric_event.is_none());
    }

    #[fuchsia::test]
    async fn convert_to_metric_uint_exponential_histogram() {
        let mut project = create_empty_project().await;
        let metric_config = create_metric_config(1, MetricType::IntHistogram);
        let hist_prop = Property::UintArray(
            "hist".to_string(),
            ArrayContent::ExponentialHistogram(ExponentialHistogram {
                floor: 0,
                initial_step: 1,
                step_multiplier: 2,
                counts: vec![1, 2, 3],
                indexes: None,
                size: 3,
            }),
        );
        let item = SelectResult::Properties(vec![Cow::Borrowed(&hist_prop)]);

        // First time, full histogram.
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        match metric_event.payload {
            MetricEventPayload::Histogram(buckets) => {
                assert_eq!(
                    buckets,
                    vec![
                        HistogramBucket { index: 0, count: 1 },
                        HistogramBucket { index: 1, count: 2 },
                        HistogramBucket { index: 2, count: 3 },
                    ]
                );
            }
            _ => panic!("Wrong payload type"),
        }

        // Second time, diff histogram.
        let updated_hist_prop = Property::UintArray(
            "hist".to_string(),
            ArrayContent::ExponentialHistogram(ExponentialHistogram {
                floor: 0,
                initial_step: 1,
                step_multiplier: 2,
                counts: vec![2, 4, 6],
                indexes: None,
                size: 3,
            }),
        );
        let item = SelectResult::Properties(vec![Cow::Borrowed(&updated_hist_prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        match metric_event.payload {
            MetricEventPayload::Histogram(buckets) => {
                assert_eq!(
                    buckets,
                    vec![
                        HistogramBucket { index: 0, count: 1 },
                        HistogramBucket { index: 1, count: 2 },
                        HistogramBucket { index: 2, count: 3 },
                    ]
                );
            }
            _ => panic!("Wrong payload type"),
        }

        // Third time, same value, should be None.
        let item = SelectResult::Properties(vec![Cow::Borrowed(&updated_hist_prop)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item, &metric_config).unwrap();
        assert!(metric_event.is_none());
    }

    #[fuchsia::test]
    async fn cache_multiple_metrics() {
        let mut project = create_empty_project().await;
        let mut metric_1 = create_metric_config(1, MetricType::Occurrence);
        metric_1.event_codes = vec![EventCode(0), EventCode(0)];

        let mut metric_2 = create_metric_config(1, MetricType::Occurrence);
        metric_2.event_codes = vec![EventCode(0), EventCode(1)];

        let prop1 = Property::Int("value".to_string(), 10);
        let item1 = SelectResult::Properties(vec![Cow::Borrowed(&prop1)]);

        let prop2 = Property::Int("value-2".to_string(), 0);
        let item2 = SelectResult::Properties(vec![Cow::Borrowed(&prop2)]);

        // First time, should be the full value.
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item1, &metric_1).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        assert_eq!(metric_event.payload, MetricEventPayload::Count(10));
        assert_eq!(metric_event.event_codes, vec![0, 0]);

        let metric_event =
            Project::convert_to_metric(&mut project.cache, item2, &metric_2).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        assert_eq!(metric_event.payload, MetricEventPayload::Count(0));
        assert_eq!(metric_event.event_codes, vec![0, 1]);

        // Second time, with a new value, should be the diff.
        let prop1 = Property::Int("value".to_string(), 15);
        let item1 = SelectResult::Properties(vec![Cow::Borrowed(&prop1)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item1, &metric_1).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        assert_eq!(metric_event.payload, MetricEventPayload::Count(5));
        assert_eq!(metric_event.event_codes, vec![0, 0]);

        let prop2 = Property::Int("value-2".to_string(), 15);
        let item2 = SelectResult::Properties(vec![Cow::Borrowed(&prop2)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item2, &metric_2).unwrap().unwrap();
        assert_eq!(metric_event.metric_id, 1);
        assert_eq!(metric_event.payload, MetricEventPayload::Count(15));
        assert_eq!(metric_event.event_codes, vec![0, 1]);

        // Third time, with same value, should be None.
        let item1 = SelectResult::Properties(vec![Cow::Borrowed(&prop1)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item1, &metric_1).unwrap();
        assert!(metric_event.is_none());

        let item2 = SelectResult::Properties(vec![Cow::Borrowed(&prop2)]);
        let metric_event =
            Project::convert_to_metric(&mut project.cache, item2, &metric_2).unwrap();
        assert!(metric_event.is_none());
    }

    #[fuchsia::test]
    async fn test_upload_once() {
        let (logger, logger_server) = create_proxy_and_stream::<MetricEventLoggerMarker>();
        let mut project = Project {
            logger,
            data_sets: vec![DataSetConfig {
                metrics: vec![
                    MetricConfig {
                        selectors: vec![parse_verbose("moniker/for/test:root:foo").unwrap()],
                        metric_id: MetricId(0),
                        metric_type: MetricType::Integer,
                        event_codes: vec![EventCode(1), EventCode(2), EventCode(3)],
                        upload_once: false,
                    },
                    MetricConfig {
                        selectors: vec![
                            parse_verbose("moniker/for/test:root:upload_once").unwrap(),
                        ],
                        metric_id: MetricId(0),
                        metric_type: MetricType::Integer,
                        event_codes: vec![EventCode(1), EventCode(2), EventCode(0)],
                        upload_once: true,
                    },
                    MetricConfig {
                        selectors: vec![
                            parse_verbose("moniker/for/test:root:upload_once_later").unwrap(),
                        ],
                        metric_id: MetricId(0),
                        metric_type: MetricType::Integer,
                        event_codes: vec![EventCode(1), EventCode(0), EventCode(0)],
                        upload_once: true,
                    },
                ],
                poll_rate_sec: 1,
            }],
            cache: HashMap::new(),
            stats: None,
        };

        let data_sink = Arc::new(Mutex::new(vec![]));
        let scope = Scope::new();
        scope.spawn(serve_mock_cobalt_logger(data_sink.clone(), logger_server));

        {
            let data = test_data(
                0,
                hierarchy! {
                    root: {
                        foo: 1i64,
                        upload_once: 10i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(0))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![
                MetricEvent {
                    metric_id: 0,
                    payload: MetricEventPayload::IntegerValue(1),
                    event_codes: vec![1, 2, 3],
                },
                MetricEvent {
                    metric_id: 0,
                    payload: MetricEventPayload::IntegerValue(10),
                    event_codes: vec![1, 2, 0],
                },
            ];

            assert_eq!(
                upload_once,
                vec![MetricConfig {
                    selectors: vec![parse_verbose("moniker/for/test:root:upload_once").unwrap()],
                    metric_id: MetricId(0),
                    metric_type: MetricType::Integer,
                    event_codes: vec![EventCode(1), EventCode(2), EventCode(0)],
                    upload_once: true,
                },]
            );
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(
                1,
                hierarchy! {
                    root: {
                        foo: 10i64,
                        upload_once_later: 0i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![
                MetricEvent {
                    metric_id: 0,
                    payload: MetricEventPayload::IntegerValue(10),
                    event_codes: vec![1, 2, 3],
                },
                MetricEvent {
                    metric_id: 0,
                    payload: MetricEventPayload::IntegerValue(0),
                    event_codes: vec![1, 0, 0],
                },
            ];

            assert_eq!(
                upload_once,
                vec![MetricConfig {
                    selectors: vec![
                        parse_verbose("moniker/for/test:root:upload_once_later").unwrap()
                    ],
                    metric_id: MetricId(0),
                    metric_type: MetricType::Integer,
                    event_codes: vec![EventCode(1), EventCode(0), EventCode(0)],
                    upload_once: true,
                },]
            );
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(
                1,
                hierarchy! {
                    root: {
                        foo: 10i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::IntegerValue(10),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(
                1,
                hierarchy! {
                    root: {
                        foo: 9i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::IntegerValue(9),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }

        {
            let data = test_data(
                1,
                hierarchy! {
                    root: {
                        foo: -9i64,
                    }
                },
            );

            let upload_once =
                project.log(&data, Some(zx::MonotonicDuration::from_seconds(1))).await.unwrap();

            let events = wait_for_data(&data_sink).await;
            let expected = vec![MetricEvent {
                metric_id: 0,
                payload: MetricEventPayload::IntegerValue(-9),
                event_codes: vec![1, 2, 3],
            }];

            assert!(upload_once.is_empty());
            assert_eq!(expected, events);
            data_sink.lock().clear();
        }
    }
}
