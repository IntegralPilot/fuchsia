// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use crate::tests::utils::{
    AssertionOption, AssertionParameters, IqueryCommand, TestComponent, new_test,
};
use assert_matches::assert_matches;
use iquery::types::Error;

// List command

#[fuchsia::test]
async fn test_list() {
    let test = new_test(&[TestComponent::Basic("basic-1"), TestComponent::Basic("basic-2")]).await;

    test.assert(AssertionParameters {
        command: IqueryCommand::List,
        golden_basename: "list_test",
        iquery_args: vec!["--accessor", "archivist:fuchsia.diagnostics.ArchiveAccessor"],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

#[fuchsia::test]
async fn test_list_no_duplicates() {
    let test = new_test(&[TestComponent::Regular("test")]).await;
    test.assert(AssertionParameters {
        command: IqueryCommand::List,
        golden_basename: "list_no_dups",
        iquery_args: vec!["--accessor", "archivist:fuchsia.diagnostics.ArchiveAccessor"],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

#[fuchsia::test]
async fn test_list_filter_component() {
    let test = new_test(&[TestComponent::Regular("test"), TestComponent::Basic("basic")]).await;
    test.assert(AssertionParameters {
        command: IqueryCommand::List,
        golden_basename: "list_filter_manifest",
        iquery_args: vec![
            "--component",
            "test_component.cm",
            "--accessor",
            "archivist:fuchsia.diagnostics.ArchiveAccessor",
        ],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

#[fuchsia::test]
async fn test_list_with_urls() {
    let test = new_test(&[TestComponent::Regular("test"), TestComponent::Basic("basic")]).await;
    test.assert(AssertionParameters {
        command: IqueryCommand::List,
        golden_basename: "list_with_url",
        iquery_args: vec![
            "--accessor",
            "archivist:fuchsia.diagnostics.ArchiveAccessor",
            "--with-url",
        ],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

#[fuchsia::test]
async fn list_archive() {
    let test = new_test(&[TestComponent::Basic("basic")]).await;
    test.assert(AssertionParameters {
        command: IqueryCommand::List,
        golden_basename: "list_archive",
        iquery_args: vec!["--accessor", "archivist:fuchsia.diagnostics.ArchiveAccessor"],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

// Selectors command

#[fuchsia::test]
async fn test_selectors_empty() {
    let test = new_test(&[]).await;
    let result = test.execute_command(&["selectors"]).await;
    assert_matches!(result, Err(Error::InvalidArguments(_)));
}

#[fuchsia::test]
async fn test_selectors() {
    let test = new_test(&[
        TestComponent::Basic("basic-1"),
        TestComponent::Basic("basic-2"),
        TestComponent::Regular("test"),
    ])
    .await;
    test.assert(AssertionParameters {
        command: IqueryCommand::Selectors,
        golden_basename: "selectors_test",
        iquery_args: vec![
            "--accessor",
            "archivist:fuchsia.diagnostics.ArchiveAccessor",
            "basic-1:root/fuchsia.inspect.Health",
            "basic-2:root",
            "test",
        ],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

#[fuchsia::test]
async fn test_selectors_filter_serve_fs() {
    let test = new_test(&[TestComponent::Basic("basic"), TestComponent::Regular("test")]).await;
    test.assert(AssertionParameters {
        command: IqueryCommand::Selectors,
        golden_basename: "selectors_filter_test_serve_fs",
        iquery_args: vec![
            "--accessor",
            "archivist:fuchsia.diagnostics.ArchiveAccessor",
            "basic_component.cm",
            "--data",
            "root/fuchsia.inspect.Health",
        ],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

#[fuchsia::test]
async fn test_selectors_filter() {
    let test = new_test(&[TestComponent::Basic("basic"), TestComponent::Regular("test")]).await;
    test.assert(AssertionParameters {
        command: IqueryCommand::Selectors,
        golden_basename: "selectors_filter_test",
        iquery_args: vec![
            "--accessor",
            "archivist:fuchsia.diagnostics.ArchiveAccessor",
            "basic_component.cm",
            "--data",
            "root/fuchsia.inspect.Health",
        ],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

// Show

#[fuchsia::test]
async fn show_test() {
    let test = new_test(&[
        TestComponent::Basic("basic-1"),
        TestComponent::Basic("basic-2"),
        TestComponent::Basic("basic-3"),
    ])
    .await;
    test.assert(AssertionParameters {
        command: IqueryCommand::Show,
        golden_basename: "show_test",
        iquery_args: vec![
            "--accessor",
            "archivist:fuchsia.diagnostics.ArchiveAccessor",
            "basic-1:root/fuchsia.inspect.Health",
            "basic-2:root:iquery",
            "basic-3",
        ],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

#[fuchsia::test]
async fn empty_result_on_null_payload() {
    let test = new_test(&[TestComponent::Basic("basic")]).await;
    let result = test.execute_command(&["show", "basic:root/nothing:here"]).await;
    assert_matches!(result, Err(_));
}

#[fuchsia::test]
async fn show_component_does_not_exist() {
    let test = new_test(&[]).await;
    let result = test
        .execute_command(&[
            "show",
            "--accessor",
            "archivist:fuchsia.diagnostics.ArchiveAccessor",
            "doesnt_exist",
        ])
        .await;
    assert_matches!(result, Ok(s) if s.is_empty());
}

#[fuchsia::test]
async fn show_filter_manifest_serve_fs() {
    let test = new_test(&[TestComponent::Basic("basic"), TestComponent::Regular("test")]).await;
    test.assert(AssertionParameters {
        command: IqueryCommand::Show,
        golden_basename: "show_filter_test_serve_fs",
        iquery_args: vec![
            "--accessor",
            "archivist:fuchsia.diagnostics.ArchiveAccessor",
            "basic_component.cm",
            "--data",
            "root/fuchsia.inspect.Health",
        ],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

#[fuchsia::test]
async fn show_filter_manifest() {
    let test = new_test(&[TestComponent::Basic("basic"), TestComponent::Regular("test")]).await;
    test.assert(AssertionParameters {
        command: IqueryCommand::Show,
        golden_basename: "show_filter_test",
        iquery_args: vec![
            "--accessor",
            "archivist:fuchsia.diagnostics.ArchiveAccessor",
            "basic_component.cm",
            "--data",
            "root/fuchsia.inspect.Health",
        ],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

#[fuchsia::test]
async fn show_filter_manifest_no_selectors_serve_fs() {
    let test = new_test(&[TestComponent::Basic("basic"), TestComponent::Regular("test")]).await;
    test.assert(AssertionParameters {
        command: IqueryCommand::Show,
        golden_basename: "show_filter_no_selectors_test_serve_fs",
        iquery_args: vec![
            "--accessor",
            "archivist:fuchsia.diagnostics.ArchiveAccessor",
            "basic_component.cm",
        ],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

#[fuchsia::test]
async fn show_filter_manifest_no_selectors() {
    let test = new_test(&[TestComponent::Basic("basic"), TestComponent::Regular("test")]).await;
    test.assert(AssertionParameters {
        command: IqueryCommand::Show,
        golden_basename: "show_filter_no_selectors_test",
        iquery_args: vec![
            "--accessor",
            "archivist:fuchsia.diagnostics.ArchiveAccessor",
            "basic_component.cm",
        ],
        opts: vec![AssertionOption::Retry],
    })
    .await;
}

#[fuchsia::test]
async fn list_accessors() {
    let test = new_test(&[]).await;
    test.assert(AssertionParameters {
        command: IqueryCommand::ListAccessors,
        golden_basename: "list_accessors",
        iquery_args: vec![],
        opts: vec![],
    })
    .await;
}
