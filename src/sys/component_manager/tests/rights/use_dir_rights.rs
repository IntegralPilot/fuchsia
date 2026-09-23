// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fidl_test_components as ftest;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use std::io;
use zx::StatusExt;

/// Contains all the information required to verify that a particular path contains some set of
/// rights.
struct RightsTestCase {
    path: &'static str,
    rights: fio::Flags,
}

impl RightsTestCase {
    /// Constructs a new rights test case.
    fn new(path: &'static str, rights: fio::Flags) -> RightsTestCase {
        RightsTestCase { path: path, rights: rights }
    }

    /// Returns every right not available for this path.
    fn unavailable_rights(&self) -> Vec<fio::Flags> {
        let all_rights = [fio::PERM_READABLE, fio::PERM_WRITABLE, fio::PERM_EXECUTABLE];

        let mut unavailable_rights: Vec<fio::Flags> = vec![];
        for right_to_check in all_rights.iter() {
            if *right_to_check | self.rights != self.rights {
                unavailable_rights.push(*right_to_check);
            }
        }
        unavailable_rights
    }

    /// Verifies that the rights are correctly implemented for the type.
    fn verify(&self) -> io::Result<()> {
        fdio::open_fd(self.path, self.rights).map_err(|s| s.into_io_error())?;
        for invalid_right in self.unavailable_rights() {
            if let Ok(_) = fdio::open_fd(self.path, invalid_right) {
                return Err(io::Error::other(format!(
                    "Path: {} with rights: {:?} was opened with invalid rights {:?}",
                    self.path, self.rights, invalid_right
                )));
            }
        }
        return Ok(());
    }
}

#[fuchsia::main]
async fn main() {
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::local(async move {
            run_trigger_service(stream).await.expect("failed to run trigger service");
        })
        .detach();
    });
    fs.take_and_serve_directory_handle().expect("failed to serve outgoing directory");
    fs.collect::<()>().await;
}

async fn run_trigger_service(mut stream: ftest::TriggerRequestStream) -> Result<(), Error> {
    while let Some(event) = stream.try_next().await? {
        handle_trigger(event).await;
    }
    Ok(())
}

async fn handle_trigger(event: ftest::TriggerRequest) {
    let ftest::TriggerRequest::Run { responder } = event;
    let tests = [
        RightsTestCase::new("/read_only", fio::PERM_READABLE),
        RightsTestCase::new("/read_write", fio::PERM_READABLE | fio::PERM_WRITABLE),
        RightsTestCase::new("/read_write_dup", fio::PERM_READABLE | fio::PERM_WRITABLE),
        RightsTestCase::new("/read_exec", fio::PERM_READABLE | fio::PERM_EXECUTABLE),
        RightsTestCase::new("/nested/read_exec", fio::PERM_READABLE | fio::PERM_EXECUTABLE),
        RightsTestCase::new("/read_only_after_scoped", fio::PERM_READABLE),
    ];
    let mut responses = vec![];
    for test_case in tests.iter() {
        if let Err(test_failure) = test_case.verify() {
            responses.push(format!(
                "Directory rights test failed: {} - {}",
                test_case.path, test_failure
            ));
        }
    }
    if responses.is_empty() {
        responses.push("All tests passed".into());
    }
    responder.send(&responses.join("\n")).expect("failed to send trigger response");
}
