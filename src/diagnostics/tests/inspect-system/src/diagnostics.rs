// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Display;
use std::sync::atomic::AtomicUsize;

static NEXT_ID: AtomicUsize = AtomicUsize::new(1000);

/// Utility for unique request identification.
///
/// A RequestId contains a connection id and, optionally, a request
/// id associated with that connection. Each identifier is unique within
/// the process. This allows individual parts of an operation to be
/// tracked throughout the overall operation's lifecycle.
#[derive(Clone, Copy)]
pub struct RequestId {
    connection_id: usize,
    request_id: Option<usize>,
}

impl Default for RequestId {
    /// Create a RequestId with a unique connection ID but no request.
    fn default() -> Self {
        Self {
            connection_id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            request_id: None,
        }
    }
}

impl RequestId {
    /// Create a RequestId with a unique connection ID but no request.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create and return a new RequestId as part of the same connection.
    pub fn new_request(&self) -> Self {
        Self {
            connection_id: self.connection_id,
            request_id: Some(NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)),
        }
    }
}

impl Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{},", self.connection_id)?;
        match self.request_id {
            Some(v) => write!(f, "{v}]"),
            None => write!(f, "xxxx]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ids_are_unique() {
        let rid = RequestId::new();
        assert!(rid.connection_id > 0);
        assert_eq!(rid.request_id, None);

        let rid2 = RequestId::new();
        assert_ne!(rid.connection_id, rid2.connection_id);

        let rid3 = rid.new_request();
        let rid4 = rid.new_request();

        assert_eq!(rid.connection_id, rid3.connection_id);
        assert_eq!(rid3.connection_id, rid4.connection_id);

        assert_ne!(rid3.request_id, rid4.request_id);
    }
}
