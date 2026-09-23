// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async as fasync;
use futures::FutureExt;

use std::future::IntoFuture;

#[fasync::run_singlethreaded]
async fn main() {
    let _task = fasync::Task::spawn(async {});
    let scope = fasync::Scope::new_with_name("scope1");
    let _scope_task = scope.spawn(baz(19));
    let block = async {
        fasync::Task::spawn(baz(20)).detach();
        let task = fasync::Task::spawn(baz(21));
        task.await;
    }
    .fuse();
    futures::pin_mut!(block);
    futures::select! {
        _ = foo().fuse() => (),
        _ = bar().fuse() => (),
        _ = block => (),
    };
}

async fn foo() {
    let scope = fasync::Scope::new_with_name("scope2");
    scope.spawn(baz(7));
    let join_handle = scope.spawn(async {
        baz(8).await;
    });
    let child = scope.new_child_with_name("scope2_child");
    child.spawn(baz(9));
    futures::join!(
        baz(10).boxed(),
        baz(11).boxed_local(),
        scope.into_future(),
        child.into_future(),
        join_handle,
    );
}

async fn bar() {
    baz(30).fuse().await;
}

async fn baz(i: i64) {
    if i == 21 {
        panic!();
    }
    fasync::Timer::new(fasync::MonotonicDuration::from_seconds(i)).await;
}
