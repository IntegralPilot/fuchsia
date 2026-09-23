// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_os = "fuchsia")]
mod target {
    use fuchsia_inspect_contrib::inspect_log;
    use fuchsia_inspect_contrib::nodes::BoundedListNode;
    use fuchsia_sync::Mutex;
    use std::sync::LazyLock;

    struct LogHolder {
        list: Mutex<Option<BoundedListNode>>,
    }

    impl LogHolder {
        fn new() -> Self {
            LogHolder { list: Mutex::new(None) }
        }
    }

    static LOGGER: LazyLock<LogHolder> = LazyLock::new(LogHolder::new);

    /// Provides a `BoundedListNode` to store logged warnings and errors in.
    pub fn set_log_list_node(node: BoundedListNode) {
        *LOGGER.list.lock() = Some(node);
    }

    /// Logs a "warn" message to a list of messages in Inspect.
    /// Until set_log_list_node() is called, this will have no effect.
    pub(crate) fn log_warn(message: &str, namespace: &str, name: &str, error: &str) {
        log_problem("warn", message, namespace, name, error);
    }

    /// Logs an "Error" message to a list of messages in Inspect.
    /// Until set_log_list_node() is called, this will have no effect.
    pub(crate) fn log_error(message: &str, namespace: &str, name: &str, error: &str) {
        log_problem("error", message, namespace, name, error);
    }

    fn log_problem(level: &str, message: &str, namespace: &str, name: &str, error: &str) {
        let Some(ref mut list) = *LOGGER.list.lock() else {
            return;
        };
        inspect_log!(
            list,
            level: level,
            message: message,
            namespace: namespace,
            name: name,
            error: error
        );
    }
}

#[cfg(target_os = "fuchsia")]
pub use target::*;

#[cfg(not(target_os = "fuchsia"))]
mod host {

    pub(crate) fn log_warn(_message: &str, _namespace: &str, _name: &str, _error: &str) {}

    pub(crate) fn log_error(_message: &str, _namespace: &str, _name: &str, _error: &str) {}
}

#[cfg(not(target_os = "fuchsia"))]
pub(crate) use host::*;
