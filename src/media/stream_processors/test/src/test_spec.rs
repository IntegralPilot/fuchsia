// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::elementary_stream::*;
use crate::output_validator::*;
use crate::stream::*;
use crate::stream_runner::*;
use crate::{FatalError, Result};
use anyhow::Context as _;
use fidl_fuchsia_media::StreamProcessorProxy;
use futures::TryStreamExt;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use std::rc::Rc;

pub enum OutputSize {
    // Size of output in terms of packets.
    PacketCount(usize),
    // Size of output in terms of number of raw bytes.
    RawBytesCount(usize),
}

const FIRST_FORMAT_DETAILS_VERSION_ORDINAL: u64 = 1;

pub type TestCaseOutputs = Vec<Output>;

pub trait StreamProcessorFactory {
    fn connect_to_stream_processor(
        &self,
        stream: &dyn ElementaryStream,
        format_details_version_ordinal: u64,
    ) -> BoxFuture<'_, Result<StreamProcessorProxy>>;
}

/// A test spec describes all the cases that will run and the circumstances in which
/// they will run.
pub struct TestSpec {
    pub cases: Vec<TestCase>,
    pub relation: CaseRelation,
    pub stream_processor_factory: Rc<dyn StreamProcessorFactory>,
}

/// A case relation describes the temporal relationship between two test cases.
pub enum CaseRelation {
    /// With serial relation, test cases will be run in sequence using the same codec server.
    /// For serial relation, outputs from test cases will be returned.
    Serial,
    /// With concurrent relation, test cases will run concurrently using two or more codec servers.
    /// For concurrent relation, outputs from test cases will not be returned.
    Concurrent,
}

/// A test cases describes a sequence of elementary stream chunks that should be fed into a codec
/// server, and a set of validators to check the output. To pass, all validations must pass for all
/// output from the stream.
pub struct TestCase {
    pub name: &'static str,
    pub stream: Rc<dyn ElementaryStream>,
    pub validators: Vec<Rc<dyn OutputValidator>>,
    pub stream_options: Option<StreamOptions>,
}

impl TestSpec {
    pub async fn run(self) -> Result<Option<Vec<TestCaseOutputs>>> {
        let res = match self.relation {
            CaseRelation::Serial => {
                Some(run_cases_serially(self.stream_processor_factory.as_ref(), self.cases).await?)
            }
            CaseRelation::Concurrent => {
                run_cases_concurrently(self.stream_processor_factory.as_ref(), self.cases).await?;
                None
            }
        };
        Ok(res)
    }
}

async fn run_cases_serially(
    stream_processor_factory: &dyn StreamProcessorFactory,
    cases: Vec<TestCase>,
) -> Result<Vec<TestCaseOutputs>> {
    let stream_processor =
        if let Some(stream) = cases.first().as_ref().map(|case| case.stream.as_ref()) {
            stream_processor_factory
                .connect_to_stream_processor(stream, FIRST_FORMAT_DETAILS_VERSION_ORDINAL)
                .await?
        } else {
            return Err(FatalError(String::from("No test cases provided.")).into());
        };
    let mut stream_runner = StreamRunner::new(stream_processor);

    let mut all_outputs = Vec::new();
    for case in cases {
        let output = stream_runner
            .run_stream(case.stream, case.stream_options.unwrap_or_default())
            .await
            .context(format!("Running case {}", case.name))?;
        for validator in case.validators {
            validator.validate(&output).await.context(format!("Validating case {}", case.name))?;
        }
        all_outputs.push(output);
    }
    Ok(all_outputs)
}

async fn run_cases_concurrently(
    stream_processor_factory: &dyn StreamProcessorFactory,
    cases: Vec<TestCase>,
) -> Result<()> {
    let mut unordered = FuturesUnordered::new();
    for case in cases {
        unordered.push(run_cases_serially(stream_processor_factory, vec![case]))
    }

    while let Some(_) = unordered.try_next().await? {}

    Ok(())
}

pub fn with_large_stack(f: fn() -> Result<()>) -> Result<()> {
    // The TestSpec futures are too big to fit on Fuchsia's default stack.
    const MEGABYTE: usize = 1024 * 1024;
    const STACK_SIZE: usize = 4 * MEGABYTE;
    std::thread::Builder::new().stack_size(STACK_SIZE).spawn(f).unwrap().join().unwrap()
}
