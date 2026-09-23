// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
#[cfg(feature = "fdomain")]
extern crate fuchsia_fuzzctl_fdomain as fuchsia_fuzzctl;

#[cfg(test)]
#[cfg(not(feature = "fdomain"))]
extern crate fuchsia_fuzzctl_test as fuchsia_fuzzctl_test;

#[cfg(test)]
#[cfg(feature = "fdomain")]
extern crate fuchsia_fuzzctl_test_fdomain as fuchsia_fuzzctl_test;

pub mod constants;

mod artifact;
mod controller;
mod corpus;
mod diagnostics;
mod duration;
mod input;
mod manager;
mod util;
mod writer;

pub use self::artifact::{Artifact, save_artifact};
pub use self::controller::Controller;
pub use self::corpus::{get_name as get_corpus_name, get_type as get_corpus_type};
pub use self::diagnostics::{Forwarder, SocketForwarder};
pub use self::duration::{MonotonicDuration, deadline_after};
pub use self::input::{Input, InputPair, save_input};
pub use self::manager::Manager;
pub use self::util::{
    create_artifact_dir, create_corpus_dir, create_dir_at, digest_path, get_fuzzer_urls,
};
pub use self::writer::{OutputSink, StdioSink, Writer};
