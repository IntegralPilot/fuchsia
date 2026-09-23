// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::*;
use component_events::matcher::*;
use component_events::sequence::*;
use fidl_fuchsia_component as fcomponent;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};

/// An integration test that runs a simple ELF component that uses a runner from
/// an ELF runner component instantiated as a child.
async fn run_test(url: &str) {
    let builder = RealmBuilder::new().await.unwrap();
    let program =
        builder.add_child("simple_elf_program", url, ChildOptions::new().eager()).await.unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.process.Launcher"))
                .from(Ref::parent())
                .to(&program),
        )
        .await
        .unwrap();
    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let proxy: fcomponent::EventStreamProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    proxy.wait_for_ready().await.unwrap();
    let event_stream = EventStream::new(proxy);

    instance.start_component_tree().await.unwrap();

    let elf_runner_start =
        EventMatcher::ok().r#type(Started::TYPE).moniker("./simple_elf_program/elf_runner");

    let simple_elf_program_start =
        EventMatcher::ok().r#type(Started::TYPE).moniker("./simple_elf_program");

    let simple_elf_program_stop =
        EventMatcher::ok().stop(Some(ExitStatusMatcher::Clean)).moniker("./simple_elf_program");

    EventSequence::new()
        .has_subset(
            vec![elf_runner_start, simple_elf_program_start, simple_elf_program_stop],
            Ordering::Ordered,
        )
        .expect(event_stream)
        .await
        .unwrap();
}

#[fuchsia::test]
async fn run_elf_component_with_elf_runner_component_from_package() {
    let url = "#meta/simple_elf_program_packaged_elf_runner.cm";
    run_test(url).await;
}

#[fuchsia::test]
async fn run_elf_component_with_elf_runner_component_from_builtin_resolver() {
    let url = "#meta/simple_elf_program_builtin_elf_runner.cm";
    run_test(url).await;
}
