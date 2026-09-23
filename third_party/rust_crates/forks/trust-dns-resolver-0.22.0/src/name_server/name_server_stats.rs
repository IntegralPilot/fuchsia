// Copyright 2015-2019 Benjamin Fry <benjaminfry@me.com>
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::cmp::Ordering;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{self, AtomicUsize};

use crate::error::ResolveErrorKind;

/// query statistics for a single name server.
#[derive(Debug, Clone)]
pub struct NameServerStats {
    /// The address of the name server.
    pub addr: std::net::SocketAddr,
    /// The protocol used to talk to the name server.
    pub proto: crate::config::Protocol,
    /// The total number of lookup failures.
    pub failures: usize,
    /// The total number of lookup successes.
    pub successes: usize,
    /// The number of successful queries since the last failure.
    pub success_streak: usize,
    /// The last `N` errors seen when querying this nameserver. Configured with
    /// [`crate::config::NameServerConfig::num_retained_errors`].
    pub recent_errors: Vec<ResolveErrorKind>,
}

#[derive(Clone)]
pub(crate) struct InternalNameServerStats {
    successes: usize,
    failures: usize,
    success_streak: usize,
    retained_errors: usize,
    recent_errors: VecDeque<ResolveErrorKind>,
    // TODO: incorporate latency
}

impl InternalNameServerStats {
    pub(crate) fn new(retained_errors: usize) -> Self {
        Self::new_internal(0, 0, 0, retained_errors)
    }

    fn new_internal(successes: usize, failures: usize, success_streak: usize, retained_errors: usize) -> Self {
        Self {
            successes: successes,
            failures: failures,
            success_streak: success_streak,
            retained_errors: retained_errors,
            recent_errors: VecDeque::with_capacity(retained_errors),
        }
    }

    pub(crate) fn next_success(&mut self) {
        self.successes += 1;
        self.success_streak += 1;
    }

    pub(crate) fn next_failure(&mut self, error: ResolveErrorKind) {
        self.failures += 1;
        self.success_streak = 0;

        if self.retained_errors > 0 {
            // Pop first so we never go above the capacity and allocate.
            if self.recent_errors.len() >= self.retained_errors {
                self.recent_errors.pop_front().unwrap();
            }
            self.recent_errors.push_back(error);
        }
    }

    pub(crate) fn export(
        &self,
        addr: std::net::SocketAddr,
        proto: crate::config::Protocol,
    ) -> NameServerStats {
        let Self { successes, failures, success_streak, retained_errors, recent_errors } = self;

        NameServerStats {
            addr,
            proto,
            failures: *failures,
            successes: *successes,
            success_streak: *success_streak,
            recent_errors: self.recent_errors.iter().cloned().collect(),
        }
    }
}

impl PartialEq for InternalNameServerStats {
    fn eq(&self, other: &Self) -> bool {
        self.successes == other.successes && self.failures == other.failures
    }
}

impl Eq for InternalNameServerStats {}

impl Ord for InternalNameServerStats {
    /// Custom implementation of Ord for NameServer which incorporates the performance of the connection into it's ranking
    fn cmp(&self, other: &Self) -> Ordering {
        // if they are literally equal, just return
        if self == other {
            return Ordering::Equal;
        }

        // TODO: track latency and use lowest latency connection...

        // invert failure comparison, i.e. the one with the least failures, wins
        if self.failures <= other.failures {
            return Ordering::Greater;
        }

        // at this point we'll go with the lesser of successes to make sure there is balance
        self.successes.cmp(&other.successes)
    }
}

impl PartialOrd for InternalNameServerStats {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_send_sync<S: Sync + Send>() -> bool {
        true
    }

    #[test]
    fn stats_are_sync() {
        assert!(is_send_sync::<InternalNameServerStats>());
    }

    #[test]
    fn test_state_cmp() {
        let nil = InternalNameServerStats::new_internal(0, 0, 0, 0);
        let successes = InternalNameServerStats::new_internal(1, 0, 0, 0);
        let failures = InternalNameServerStats::new_internal(0, 1, 0, 0);

        assert_eq!(nil.cmp(&nil), Ordering::Equal);
        assert_eq!(nil.cmp(&successes), Ordering::Greater);
        assert_eq!(successes.cmp(&failures), Ordering::Greater);
    }
}
