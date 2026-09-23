// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::inspect_util;
use fuchsia_inspect::Node;
use fuchsia_sync::Mutex;
use fuchsia_url::fuchsia_pkg::AbsolutePackageUrl;
use futures::future::BoxFuture;
use {fidl_fuchsia_pkg as fpkg, fidl_fuchsia_pkg_ext as pkg};

const SUCCESSFUL_RESOLVE_HISTORY: usize = 100;
// Subpackage resolves never require network interactions or writing to storage and should always
// succeed (unless the request is bad), so we don't need to save as many.
const SUCCESSFUL_SUBPACKAGE_RESOLVE_HISTORY: usize = 50;

fn now_monotonic_nanos() -> i64 {
    zx::MonotonicInstant::get().into_nanos()
}

/// Wraps the Inspect state of package resolves.
#[derive(Debug)]
pub struct ResolverService {
    /// How many times the resolver service has fallen back to the
    /// cache package set due to a remote repository returning NOT_FOUND.
    /// TODO(https://fxbug.dev/42127880): remove this stat when we remove this cache fallback
    /// behavior.
    cache_fallbacks_due_to_not_found: inspect_util::Counter,
    active_package_resolves: Node,
    successful_resolves: Mutex<fuchsia_inspect_contrib::nodes::BoundedListNode>,
    successful_subpackage_resolves: Mutex<fuchsia_inspect_contrib::nodes::BoundedListNode>,
    _node: Node,
}

impl ResolverService {
    /// Make a `ResolverService` from an Inspect `Node`.
    pub fn from_node(node: Node) -> Self {
        Self {
            cache_fallbacks_due_to_not_found: inspect_util::Counter::new(
                &node,
                "cache_fallbacks_due_to_not_found",
            ),
            active_package_resolves: node.create_child("active_package_resolves"),
            successful_resolves: Mutex::new(fuchsia_inspect_contrib::nodes::BoundedListNode::new(
                node.create_child("successful_resolves"),
                SUCCESSFUL_RESOLVE_HISTORY,
            )),
            successful_subpackage_resolves: Mutex::new(
                fuchsia_inspect_contrib::nodes::BoundedListNode::new(
                    node.create_child("successful_subpackage_resolves"),
                    SUCCESSFUL_SUBPACKAGE_RESOLVE_HISTORY,
                ),
            ),
            _node: node,
        }
    }

    /// Increment the count of package resolves that have fallen back to cache packages due to a
    /// remote repository returning NOT_FOUND. This fallback behavior will be removed
    /// TODO(https://fxbug.dev/42127880).
    pub fn cache_fallback_due_to_not_found(&self) {
        self.cache_fallbacks_due_to_not_found.increment();
    }

    /// Add a package to the list of active resolves.
    pub fn resolve(
        &self,
        original_url: &AbsolutePackageUrl,
        gc_protection: fpkg::GcProtection,
    ) -> Package {
        let node = self.active_package_resolves.create_child(original_url.to_string());
        node.record_int("resolve_ts", now_monotonic_nanos());
        node.record_string("gc_protection", format!("{gc_protection:?}"));
        Package { node }
    }

    /// Add a child node for the raw WorkQueue underlying the QueuedResolver.
    pub fn record_raw_queue(
        &self,
        lazy_callback: impl Fn()
            -> BoxFuture<'static, Result<fuchsia_inspect::Inspector, anyhow::Error>>
        + Send
        + Sync
        + 'static,
    ) {
        let () = self._node.record_lazy_child("raw_queue", lazy_callback);
    }

    /// Record a successful package resolve in the rolling log.
    pub fn successful_resolve(
        &self,
        source: &str,
        requested_url: &AbsolutePackageUrl,
        rewritten_url: Option<&AbsolutePackageUrl>,
        gc_protection: fpkg::GcProtection,
        intermediate_error: Option<anyhow::Error>,
        blob: &pkg::BlobId,
        start_ts: zx::BootInstant,
    ) {
        self.successful_resolves.lock().add_entry(|node| {
            node.record_string("source", source);
            node.record_string("requested_url", requested_url.to_string());
            if let Some(rewritten_url) = rewritten_url {
                node.record_string("rewritten_url", rewritten_url.to_string());
            }
            node.record_string(
                "gc_protection",
                match gc_protection {
                    fpkg::GcProtection::OpenPackageTracking => "open package tracking",
                    fpkg::GcProtection::Retained => "retained index",
                },
            );
            if let Some(intermediate_error) = intermediate_error {
                node.record_string("intermediate_error", format!("{intermediate_error:#}"));
            }
            node.record_string("hash", blob.to_string());
            node.record_int("end_boot_ns", zx::BootInstant::get().into_nanos());
            node.record_int("start_boot_ns", start_ts.into_nanos())
        });
    }

    /// Record a successful subpackage package resolve in the rolling log.
    pub fn successful_subpackage_resolve(
        &self,
        url: &fuchsia_url::RelativePackageUrl,
        context: Option<&fidl_fuchsia_pkg_ext::BlobId>,
        start_ts: zx::BootInstant,
    ) {
        self.successful_subpackage_resolves.lock().add_entry(|node| {
            node.record_string("requested_url", url.to_string());
            node.record_string("context", format!("{:?}", context));
            node.record_int("end_boot_ns", zx::BootInstant::get().into_nanos());
            node.record_int("start_boot_ns", start_ts.into_nanos())
        });
    }
}

/// A package that is actively being resolved.
pub struct Package {
    node: Node,
}

impl Package {
    /// Export the package's rewritten url.
    pub fn rewritten_url(self, rewritten_url: &AbsolutePackageUrl) -> PackageWithRewrittenUrl {
        self.node.record_string("rewritten_url", rewritten_url.to_string());
        PackageWithRewrittenUrl { _node: self.node }
    }
}

/// A package with a rewritten url that is actively being resolved.
pub struct PackageWithRewrittenUrl {
    _node: Node,
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use fuchsia_inspect::Inspector;

    #[fuchsia::test]
    async fn package_state_progression() {
        let inspector = Inspector::default();

        let resolver_service =
            ResolverService::from_node(inspector.root().create_child("resolver_service"));
        assert_data_tree!(
            inspector,
            root: {
                resolver_service: contains {
                    active_package_resolves: {}
                }
            }
        );

        let package = resolver_service.resolve(
            &"fuchsia-pkg://example.org/name".parse().unwrap(),
            fpkg::GcProtection::Retained,
        );
        assert_data_tree!(
            inspector,
            root: {
                resolver_service: contains {
                    active_package_resolves: {
                        "fuchsia-pkg://example.org/name": {
                            resolve_ts: AnyProperty,
                            gc_protection: "Retained",
                        }
                    }
                }
            }
        );

        let _package =
            package.rewritten_url(&"fuchsia-pkg://rewritten.example.org/name".parse().unwrap());
        assert_data_tree!(
            inspector,
            root: {
                resolver_service: contains {
                    active_package_resolves: {
                        "fuchsia-pkg://example.org/name": {
                            resolve_ts: AnyProperty,
                            gc_protection: "Retained",
                            rewritten_url: "fuchsia-pkg://rewritten.example.org/name",
                        }
                    }
                }
            }
        );
    }

    #[fuchsia::test]
    async fn concurrent_resolves() {
        let inspector = Inspector::default();
        let resolver_service =
            ResolverService::from_node(inspector.root().create_child("resolver_service"));

        let _package0 = resolver_service.resolve(
            &"fuchsia-pkg://example.org/name".parse().unwrap(),
            fpkg::GcProtection::Retained,
        );
        let _package1 = resolver_service.resolve(
            &"fuchsia-pkg://example.org/other".parse().unwrap(),
            fpkg::GcProtection::OpenPackageTracking,
        );
        assert_data_tree!(
            inspector,
            root: {
                resolver_service: contains {
                    active_package_resolves: {
                        "fuchsia-pkg://example.org/name": contains {
                            gc_protection: "Retained",
                        },
                        "fuchsia-pkg://example.org/other": contains {
                            gc_protection: "OpenPackageTracking",
                        }
                    }
                }
            }
        );
    }

    #[fuchsia::test]
    async fn cache_fallback_due_to_not_found_increments() {
        let inspector = Inspector::default();

        let resolver_service =
            ResolverService::from_node(inspector.root().create_child("resolver_service"));
        assert_data_tree!(
            inspector,
            root: {
                resolver_service: contains {
                    cache_fallbacks_due_to_not_found: 0u64,
                }
            }
        );

        resolver_service.cache_fallback_due_to_not_found();
        assert_data_tree!(
            inspector,
            root: {
                resolver_service: contains {
                    cache_fallbacks_due_to_not_found: 1u64,
                }
            }
        );
    }

    #[fuchsia::test]
    async fn successful_resolve() {
        let inspector = Inspector::default();
        let resolver_service =
            ResolverService::from_node(inspector.root().create_child("resolver_service"));

        resolver_service.successful_resolve(
            "source0",
            &"fuchsia-pkg://example.org/package0".parse().unwrap(),
            None,
            fpkg::GcProtection::OpenPackageTracking,
            None,
            &[0; 32].into(),
            zx::BootInstant::from_nanos(0),
        );
        assert_data_tree!(
            inspector,
            root: {
                resolver_service: contains {
                    successful_resolves: {
                        "0": {
                            "source": "source0",
                            "requested_url": "fuchsia-pkg://example.org/package0",
                            "gc_protection": "open package tracking",
                            "hash":
                                "0000000000000000000000000000000000000000000000000000000000000000",
                            "start_boot_ns": AnyProperty,
                            "end_boot_ns": AnyProperty,
                        }
                    }
                }
            }
        );

        resolver_service.successful_resolve(
            "source1",
            &"fuchsia-pkg://example.org/package1".parse().unwrap(),
            Some(&"fuchsia-pkg://example.com/package1".parse().unwrap()),
            fpkg::GcProtection::Retained,
            Some(anyhow::anyhow!("i goofed")),
            &[1; 32].into(),
            zx::BootInstant::from_nanos(0),
        );
        assert_data_tree!(
            inspector,
            root: {
                resolver_service: contains {
                    successful_resolves: {
                        "0": {
                            "source": "source0",
                            "requested_url": "fuchsia-pkg://example.org/package0",
                            "gc_protection": "open package tracking",
                            "hash":
                                "0000000000000000000000000000000000000000000000000000000000000000",
                            "start_boot_ns": AnyProperty,
                            "end_boot_ns": AnyProperty,
                        },
                        "1": {
                            "source": "source1",
                            "requested_url": "fuchsia-pkg://example.org/package1",
                            "rewritten_url": "fuchsia-pkg://example.com/package1",
                            "gc_protection": "retained index",
                            "intermediate_error": "i goofed",
                            "hash":
                                "0101010101010101010101010101010101010101010101010101010101010101",
                            "start_boot_ns": AnyProperty,
                            "end_boot_ns": AnyProperty,
                        }
                    }
                }
            }
        );
    }

    #[fuchsia::test]
    async fn successful_subpackage_resolve() {
        let inspector = Inspector::default();
        let resolver_service =
            ResolverService::from_node(inspector.root().create_child("resolver_service"));

        resolver_service.successful_subpackage_resolve(
            &"the-package-name".parse().unwrap(),
            Some(&[0; 32].into()),
            zx::BootInstant::from_nanos(3),
        );

        assert_data_tree!(
            inspector,
            root: {
                resolver_service: contains {
                    successful_subpackage_resolves: {
                        "0": {
                            "requested_url": "the-package-name",
                            "context":
                                "Some(\
                                 0000000000000000000000000000000000000000000000000000000000000000\
                                 )",
                            "start_boot_ns": 3i64,
                            "end_boot_ns": AnyProperty,
                        }
                    }
                }
            }
        );
    }
}
