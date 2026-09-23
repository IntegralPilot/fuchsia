// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use event_listener::Event;
use fidl_fuchsia_fuzzer as fuzz;
use std::cell::RefCell;
use std::rc::Rc;

/// Wrapper for `fuchsia.fuzzer.Options` that can be awaited on until set.
///
/// Example:
/// ```
///     let getter = AsyncOptions::new();
///     let setter = getter.clone();
///     let get_fut = async move {
///         let options = getter.get().await;
///         ...
///     };
///     let set_fut = async move {
///         let options = ...
///         setter.set(options);
///     };
///     futures::join!(get_fut, set_fut);
/// ```
#[derive(Debug, Clone)]
pub struct AsyncOptions {
    options: Rc<RefCell<Option<fuzz::Options>>>,
    event: Rc<Event>,
    num_listeners: Rc<RefCell<usize>>,
}

impl AsyncOptions {
    /// Creates a new async options wrapper.
    pub fn new() -> Self {
        Self {
            options: Rc::new(RefCell::new(None)),
            event: Rc::new(Event::new()),
            num_listeners: Rc::new(RefCell::new(0)),
        }
    }

    /// Returns the options.
    ///
    /// The `Future` returned by this call will not complete until a call to `set` has been made.
    /// If `set` has already been called, returns immediately.
    pub async fn get(&self) -> fuzz::Options {
        if self.options.borrow().is_none() {
            self.num_listeners.replace_with(|&mut prev| prev + 1);
            self.event.listen().await;
        }
        self.options.borrow().clone().unwrap()
    }

    /// Sets the options wrapped by this object.
    ///
    /// Only the fields that are set in the given `options` will be updated. If this is the first
    /// call to `set`, any pending Futures  returned by `get` will be awoken.
    pub fn set(&self, options: fuzz::Options) {
        let mut options_mut = self.options.borrow_mut();
        let mut opts = options_mut.take().unwrap_or_else(|| fuzz::Options { ..Default::default() });
        opts.runs = options.runs.or(opts.runs);
        opts.max_total_time = options.max_total_time.or(opts.max_total_time);
        opts.seed = options.seed.or(opts.seed);
        opts.max_input_size = options.max_input_size.or(opts.max_input_size);
        opts.mutation_depth = options.mutation_depth.or(opts.mutation_depth);
        opts.dictionary_level = options.dictionary_level.or(opts.dictionary_level);
        opts.detect_exits = options.detect_exits.or(opts.detect_exits);
        opts.detect_leaks = options.detect_leaks.or(opts.detect_leaks);
        opts.run_limit = options.run_limit.or(opts.run_limit);
        opts.malloc_limit = options.malloc_limit.or(opts.malloc_limit);
        opts.oom_limit = options.oom_limit.or(opts.oom_limit);
        opts.purge_interval = options.purge_interval.or(opts.purge_interval);
        opts.malloc_exitcode = options.malloc_exitcode.or(opts.malloc_exitcode);
        opts.death_exitcode = options.death_exitcode.or(opts.death_exitcode);
        opts.leak_exitcode = options.leak_exitcode.or(opts.leak_exitcode);
        opts.oom_exitcode = options.oom_exitcode.or(opts.oom_exitcode);
        opts.pulse_interval = options.pulse_interval.or(opts.pulse_interval);
        opts.debug = options.debug.or(opts.debug);
        opts.print_final_stats = options.print_final_stats.or(opts.print_final_stats);
        opts.use_value_profile = options.use_value_profile.or(opts.use_value_profile);
        opts.sanitizer_options = options.sanitizer_options.or(opts.sanitizer_options);
        *options_mut = Some(opts);
        self.event.notify(*self.num_listeners.borrow());
    }
}

#[cfg(test)]
mod tests {
    use super::AsyncOptions;
    use futures::join;
    use {fidl_fuchsia_fuzzer as fuzz, fuchsia_async as fasync};

    #[fuchsia::test]
    async fn test_async_options_set_all() {
        let async_options = AsyncOptions::new();

        // Set all options.
        let options = fuzz::Options {
            runs: Some(100),
            max_total_time: Some(2000000),
            seed: Some(3),
            max_input_size: Some(400),
            mutation_depth: Some(5),
            dictionary_level: Some(6),
            detect_exits: Some(false),
            detect_leaks: Some(true),
            run_limit: Some(7000),
            malloc_limit: Some(8000),
            oom_limit: Some(9000),
            purge_interval: Some(10000),
            malloc_exitcode: Some(1011),
            death_exitcode: Some(1012),
            leak_exitcode: Some(1013),
            oom_exitcode: Some(1014),
            pulse_interval: Some(15000),
            debug: Some(false),
            print_final_stats: Some(true),
            use_value_profile: Some(false),
            sanitizer_options: Some(fuzz::SanitizerOptions {
                name: "foo".to_string(),
                value: "bar".to_string(),
            }),
            ..Default::default()
        };
        async_options.set(options);

        // Check all options.
        let options = async_options.get().await;
        assert_eq!(options.runs, Some(100));
        assert_eq!(options.max_total_time, Some(2000000));
        assert_eq!(options.seed, Some(3));
        assert_eq!(options.max_input_size, Some(400));
        assert_eq!(options.mutation_depth, Some(5));
        assert_eq!(options.dictionary_level, Some(6));
        assert_eq!(options.detect_exits, Some(false));
        assert_eq!(options.detect_leaks, Some(true));
        assert_eq!(options.run_limit, Some(7000));
        assert_eq!(options.malloc_limit, Some(8000));
        assert_eq!(options.oom_limit, Some(9000));
        assert_eq!(options.purge_interval, Some(10000));
        assert_eq!(options.malloc_exitcode, Some(1011));
        assert_eq!(options.death_exitcode, Some(1012));
        assert_eq!(options.leak_exitcode, Some(1013));
        assert_eq!(options.oom_exitcode, Some(1014));
        assert_eq!(options.pulse_interval, Some(15000));
        assert_eq!(options.debug, Some(false));
        assert_eq!(options.print_final_stats, Some(true));
        assert_eq!(options.use_value_profile, Some(false));
        assert_eq!(
            options.sanitizer_options,
            Some(fuzz::SanitizerOptions { name: "foo".to_string(), value: "bar".to_string() })
        );
    }

    #[fuchsia::test]
    async fn test_async_options_set_twice() {
        let async_options = AsyncOptions::new();

        // Set some options.
        let options = fuzz::Options {
            runs: Some(100),
            max_total_time: Some(2000000),
            seed: Some(3),
            max_input_size: Some(400),
            ..Default::default()
        };
        async_options.set(options);
        let options = async_options.get().await;
        assert_eq!(options.runs, Some(100));
        assert_eq!(options.max_total_time, Some(2000000));
        assert_eq!(options.seed, Some(3));
        assert_eq!(options.max_input_size, Some(400));
        assert_eq!(options.mutation_depth, None);

        // Set options again and change some values.
        let options = fuzz::Options {
            seed: Some(4),
            max_input_size: Some(500),
            mutation_depth: Some(6),
            ..Default::default()
        };
        async_options.set(options);
        let options = async_options.get().await;
        assert_eq!(options.runs, Some(100));
        assert_eq!(options.max_total_time, Some(2000000));
        assert_eq!(options.seed, Some(4));
        assert_eq!(options.max_input_size, Some(500));
        assert_eq!(options.mutation_depth, Some(6));
    }

    #[fuchsia::test]
    async fn test_async_options_delayed_set() {
        let async_options = AsyncOptions::new();
        let options_to_set = async_options.clone();
        let options_to_get = async_options.clone();

        // Try to get options, and then set them after a delay.
        let delayed_set = async move {
            fasync::Timer::new(fasync::MonotonicInstant::after(
                zx::MonotonicDuration::from_millis(10),
            ))
            .await;
            let options = fuzz::Options {
                runs: Some(100),
                max_total_time: Some(2000000),
                seed: Some(3),
                ..Default::default()
            };
            options_to_set.set(options);
        };
        let (_, options) = join!(delayed_set, options_to_get.get());
        assert_eq!(options.runs, Some(100));
        assert_eq!(options.max_total_time, Some(2000000));
        assert_eq!(options.seed, Some(3));
        assert_eq!(options.max_input_size, None);

        // Get options again. Values should be available immediately.
        let options = async_options.get().await;
        assert_eq!(options.runs, Some(100));
        assert_eq!(options.max_total_time, Some(2000000));
        assert_eq!(options.seed, Some(3));
        assert_eq!(options.max_input_size, None);
    }
}
