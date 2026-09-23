// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async::Task;

pub mod fake_object;
pub mod writer;

// The executor won't immediately wake threads to run tasks in parallel.  This
// is a hack that should force the executor to use multiple threads to run
// tasks.  Returns a vector of tasks that ensures the threads continue running.
pub async fn force_executor_threads_to_run(num_threads: usize) -> Vec<Task<()>> {
    let mut tasks = Vec::new();

    // Spawn tasks that continually yield.
    for _ in 1..num_threads {
        tasks.push(Task::spawn(async {
            loop {
                fuchsia_async::yield_now().await;
            }
        }));
    }

    // Loop for a while to make sure the threads are running before we return.
    for _ in 0..32 * num_threads {
        fuchsia_async::yield_now().await;
    }

    tasks
}
