// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::identity::ComponentIdentity;
use crate::inspect::container::{
    InspectArtifactsContainer, InspectHandle, UnpopulatedInspectDataContainer,
};
use crate::pipeline::{Pipeline, StaticHierarchyAllowlist};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_diagnostics::Selector;
use fidl_fuchsia_inspect as finspect;
use flyweights::FlyStr;
use fuchsia_async as fasync;
use fuchsia_sync::{RwLock, RwLockWriteGuard};
use log::{debug, warn};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Weak};

static INSPECT_ESCROW_NAME: zx::Name = zx::Name::new_lossy("InspectEscrowedVmo");

pub struct InspectRepository {
    inner: RwLock<InspectRepositoryInner>,
    pipelines: Vec<Weak<Pipeline>>,
    scope: fasync::Scope,
}

impl InspectRepository {
    pub fn new(pipelines: Vec<Weak<Pipeline>>, scope: fasync::Scope) -> InspectRepository {
        Self {
            pipelines,
            scope,
            inner: RwLock::new(InspectRepositoryInner { diagnostics_containers: HashMap::new() }),
        }
    }

    /// Return all the containers that contain Inspect hierarchies which contain data that should
    /// be selected from.
    pub fn fetch_inspect_data(
        &self,
        component_selectors: &Option<Vec<Selector>>,
        static_allowlist: StaticHierarchyAllowlist,
    ) -> Vec<UnpopulatedInspectDataContainer> {
        self.inner.read().fetch_inspect_data(component_selectors, static_allowlist)
    }

    fn add_inspect_artifacts(
        self: &Arc<Self>,
        mut guard: RwLockWriteGuard<'_, InspectRepositoryInner>,
        identity: Arc<ComponentIdentity>,
        proxy_handle: InspectHandle,
        remove_associated: Option<zx::Koid>,
    ) {
        // insert_inspect_artifact_container returns None when we were already tracking the
        // directory for this component. If that's the case we can return early.
        let weak_clone = Arc::downgrade(self);
        let identity_clone = Arc::clone(&identity);
        let Some(cleanup_task) = guard.insert_inspect_artifact_container(
            Arc::clone(&identity),
            proxy_handle,
            remove_associated,
            move |koid| {
                if let Some(this) = weak_clone.upgrade() {
                    this.on_handle_closed(koid, identity_clone);
                }
            },
        ) else {
            // If we got None, it means that this was already tracked and there's nothing to do.
            return;
        };

        self.scope.spawn(cleanup_task);

        // Let each pipeline know that a new component arrived, and allow the pipeline
        // to eagerly bucket static selectors based on that component's moniker.
        for pipeline_weak in self.pipelines.iter() {
            if let Some(pipeline) = pipeline_weak.upgrade() {
                pipeline.add_component(&identity.moniker).unwrap_or_else(|err| {
                    warn!(identity:%, err:?;
                                "Failed to add inspect artifacts to pipeline wrapper");
                });
            }
        }
    }

    pub(crate) fn escrow_handle<T: Into<FlyStr>>(
        self: &Arc<Self>,
        component: Arc<ComponentIdentity>,
        vmo: zx::Vmo,
        token: finspect::EscrowToken,
        name: Option<T>,
        tree: Option<zx::Koid>,
    ) {
        debug!(identity:% = component; "Escrow inspect handle.");
        if let Err(err) = vmo.set_name(&INSPECT_ESCROW_NAME) {
            debug!(err:%; "Failed to set escrow vmo name");
        }
        let handle = InspectHandle::escrow(vmo, token, name);
        let guard = self.inner.write();
        self.add_inspect_artifacts(guard, Arc::clone(&component), handle, tree);
    }

    pub(crate) fn fetch_escrow(
        self: &Arc<Self>,
        component: Arc<ComponentIdentity>,
        token: finspect::EscrowToken,
        tree: Option<ClientEnd<finspect::TreeMarker>>,
    ) -> Option<zx::Vmo> {
        debug!(identity:% = component; "Fetch Escrowed inspect handle.");
        let koid = token.token.as_handle_ref().koid().unwrap();
        let mut guard = self.inner.write();
        let container = guard.diagnostics_containers.get_mut(&component)?;
        let (handle, _) = container.remove_handle(koid);
        let handle = handle?;
        let InspectHandle::Escrow { vmo, name, .. } = handle.as_ref() else {
            return None;
        };
        if let Some(tree) = tree {
            self.add_inspect_artifacts(
                guard,
                component,
                InspectHandle::tree(tree.into_proxy(), name.clone()),
                None,
            );
        }
        Some(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
    }

    pub(crate) fn add_inspect_handle(
        self: &Arc<Self>,
        component: Arc<ComponentIdentity>,
        handle: InspectHandle,
    ) {
        debug!(identity:% = component; "Added inspect handle.");
        let guard = self.inner.write();
        self.add_inspect_artifacts(guard, Arc::clone(&component), handle, None);
    }

    fn on_handle_closed(&self, koid_to_remove: zx::Koid, identity: Arc<ComponentIdentity>) {
        // Hold the lock while we remove and update pipelines.
        let mut guard = self.inner.write();

        if let Some(container) = guard.diagnostics_containers.get_mut(&identity)
            && container.remove_handle(koid_to_remove).1 != 0
        {
            return;
        }

        guard.diagnostics_containers.remove(&identity);

        for pipeline_weak in &self.pipelines {
            if let Some(pipeline) = pipeline_weak.upgrade() {
                pipeline.remove_component(&identity.moniker);
            }
        }
    }
}

#[cfg(test)]
impl InspectRepository {
    pub(crate) fn terminate_inspect(&self, identity: Arc<ComponentIdentity>) {
        self.inner.write().diagnostics_containers.remove(&identity);
    }

    fn has_match(&self, identity: &Arc<ComponentIdentity>) -> bool {
        let lock = self.inner.read();
        lock.get_diagnostics_containers().get(identity).is_some()
    }

    /// Wait for data to appear for `identity`. Will run indefinitely if no data shows up.
    pub(crate) async fn wait_for_artifact(&self, identity: &Arc<ComponentIdentity>) {
        loop {
            if self.has_match(identity) {
                return;
            }

            fasync::Timer::new(zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(
                100,
            )))
            .await;
        }
    }

    /// Wait until nothing is present for `identity`. Will run indefinitely if data persists.
    pub(crate) async fn wait_until_gone(&self, identity: &Arc<ComponentIdentity>) {
        loop {
            if !self.has_match(identity) {
                return;
            }

            fasync::Timer::new(zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(
                100,
            )))
            .await;
        }
    }
}

struct InspectRepositoryInner {
    /// All the diagnostics directories that we are tracking.
    diagnostics_containers: HashMap<Arc<ComponentIdentity>, InspectArtifactsContainer>,
}

impl InspectRepositoryInner {
    // Inserts an InspectArtifactsContainer into the data repository.
    fn insert_inspect_artifact_container<T: FnOnce(zx::Koid)>(
        &mut self,
        identity: Arc<ComponentIdentity>,
        proxy_handle: InspectHandle,
        remove_associated: Option<zx::Koid>,
        on_closed: T,
    ) -> Option<impl Future<Output = ()> + use<T>> {
        let mut diag_repo_entry_opt = self.diagnostics_containers.get_mut(&identity);
        match diag_repo_entry_opt {
            None => {
                let mut inspect_container = InspectArtifactsContainer::default();
                let fut = inspect_container.push_handle(proxy_handle, on_closed);
                self.diagnostics_containers.insert(identity, inspect_container);
                fut
            }
            Some(ref mut artifacts_container) => {
                // When we escrow a vmo handle and provide an associated tree koid, we want to
                // ensure we atomically insert and remove. That's why we provide this optional koid
                // here and remove it under the same lock as the insertion of the escrowed data.
                if let Some(koid) = remove_associated {
                    artifacts_container.remove_handle(koid);
                }
                artifacts_container.push_handle(proxy_handle, on_closed)
            }
        }
    }

    fn fetch_inspect_data(
        &self,
        all_dynamic_selectors: &Option<Vec<Selector>>,
        static_allowlist: StaticHierarchyAllowlist,
    ) -> Vec<UnpopulatedInspectDataContainer> {
        let mut containers = vec![];
        for (identity, container) in self.diagnostics_containers.iter() {
            let component_allowlist = static_allowlist.get(&identity.moniker);

            // This artifact contains inspect and matches a passed selector.
            if let Some(unpopulated) =
                container.create_unpopulated(identity, component_allowlist, all_dynamic_selectors)
            {
                containers.push(unpopulated);
            }
        }
        containers
    }
}

#[cfg(test)]
impl InspectRepositoryInner {
    pub(crate) fn get(
        &self,
        identity: &Arc<ComponentIdentity>,
    ) -> Option<&InspectArtifactsContainer> {
        self.diagnostics_containers.get(identity)
    }

    pub(crate) fn get_diagnostics_containers(
        &self,
    ) -> &HashMap<Arc<ComponentIdentity>, InspectArtifactsContainer> {
        &self.diagnostics_containers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use fidl::AsHandleRef;
    use fidl::endpoints::Proxy;
    use fidl_fuchsia_inspect as finspect;
    use fuchsia_inspect::{Inspector, InspectorConfig};
    use moniker::ExtendedMoniker;
    use selectors::FastError;
    use std::sync::LazyLock;

    const TEST_URL: &str = "fuchsia-pkg://test";
    static ESCROW_TEST_RIGHTS: LazyLock<zx::Rights> = LazyLock::new(|| {
        zx::Rights::BASIC | zx::Rights::READ | zx::Rights::MAP | zx::Rights::PROPERTY
    });

    #[fuchsia::test]
    fn inspect_repo_disallows_duplicated_handles() {
        let _exec = fuchsia_async::LocalExecutor::default();
        let inspect_repo = Arc::new(InspectRepository::new(vec![], fasync::Scope::new()));
        let moniker = ExtendedMoniker::parse_str("./a/b/foo").unwrap();
        let identity = Arc::new(ComponentIdentity::new(moniker, TEST_URL));

        let (proxy, _stream) = fidl::endpoints::create_proxy::<finspect::TreeMarker>();
        let proxy_clone = proxy.clone();
        inspect_repo
            .add_inspect_handle(Arc::clone(&identity), InspectHandle::tree(proxy, Some("test")));

        inspect_repo.add_inspect_handle(
            Arc::clone(&identity),
            InspectHandle::tree(proxy_clone, Some("test")),
        );

        let guard = inspect_repo.inner.read();
        let container = guard.get(&identity).unwrap();
        assert_eq!(container.handles().len(), 1);
    }

    #[fuchsia::test]
    async fn repo_removes_entries_when_inspect_is_disconnected() {
        let data_repo = Arc::new(InspectRepository::new(vec![], fasync::Scope::new()));
        let moniker = ExtendedMoniker::parse_str("./a/b/foo").unwrap();
        let identity = Arc::new(ComponentIdentity::new(moniker, TEST_URL));
        let (proxy, server_end) = fidl::endpoints::create_proxy::<finspect::TreeMarker>();
        data_repo
            .add_inspect_handle(Arc::clone(&identity), InspectHandle::tree(proxy, Some("test")));
        assert!(data_repo.inner.read().get(&identity).is_some());
        drop(server_end);
        while data_repo.inner.read().get(&identity).is_some() {
            fasync::Timer::new(fasync::MonotonicInstant::after(
                zx::MonotonicDuration::from_millis(100_i64),
            ))
            .await;
        }
    }

    #[fuchsia::test]
    async fn related_handle_closes_when_repo_handle_is_removed() {
        let repo = Arc::new(InspectRepository::new(vec![], fasync::Scope::new()));
        let identity = Arc::new(ComponentIdentity::unknown());
        let (proxy, server_end) = fidl::endpoints::create_proxy::<finspect::TreeMarker>();
        let koid = proxy.as_channel().as_handle_ref().koid().unwrap();
        repo.add_inspect_handle(Arc::clone(&identity), InspectHandle::tree(proxy, Some("test")));
        {
            let mut guard = repo.inner.write();
            let container = guard.diagnostics_containers.get_mut(&identity).unwrap();
            container.remove_handle(koid);
        }
        fasync::Channel::from_channel(server_end.into_channel()).on_closed().await.unwrap();
    }

    #[fuchsia::test]
    async fn repo_integrates_with_the_pipeline() {
        let selector = selectors::parse_selector::<FastError>(r#"a/b/foo:root"#).unwrap();
        let static_selectors_opt = Some(vec![selector]);
        let pipeline = Arc::new(Pipeline::for_test(static_selectors_opt));
        let data_repo =
            Arc::new(InspectRepository::new(vec![Arc::downgrade(&pipeline)], fasync::Scope::new()));
        let moniker = ExtendedMoniker::parse_str("./a/b/foo").unwrap();
        let identity = Arc::new(ComponentIdentity::new(moniker.clone(), TEST_URL));
        let (proxy, server_end) = fidl::endpoints::create_proxy::<finspect::TreeMarker>();
        data_repo
            .add_inspect_handle(Arc::clone(&identity), InspectHandle::tree(proxy, Some("test")));
        assert!(data_repo.inner.read().get(&identity).is_some());
        assert!(pipeline.static_hierarchy_allowlist().component_was_added(&moniker));

        // When the directory disconnects, both the pipeline matchers and the repo are cleaned
        drop(server_end);
        while data_repo.inner.read().get(&identity).is_some() {
            fasync::Timer::new(fasync::MonotonicInstant::after(
                zx::MonotonicDuration::from_millis(100_i64),
            ))
            .await;
        }

        assert!(!pipeline.static_hierarchy_allowlist().component_was_added(&moniker));
    }

    #[fuchsia::test]
    fn data_repo_filters_inspect_by_selectors() {
        let _exec = fuchsia_async::LocalExecutor::default();
        let data_repo = Arc::new(InspectRepository::new(vec![], fasync::Scope::new()));
        let moniker = ExtendedMoniker::parse_str("./a/b/foo").unwrap();
        let identity = Arc::new(ComponentIdentity::new(moniker, TEST_URL));

        let (proxy, _server_end) = fidl::endpoints::create_proxy::<finspect::TreeMarker>();
        data_repo
            .add_inspect_handle(Arc::clone(&identity), InspectHandle::tree(proxy, Some("test")));

        let moniker2 = ExtendedMoniker::parse_str("./a/b/foo2").unwrap();
        let identity2 = Arc::new(ComponentIdentity::new(moniker2, TEST_URL));
        let (proxy, _server_end) = fidl::endpoints::create_proxy::<finspect::TreeMarker>();
        data_repo
            .add_inspect_handle(Arc::clone(&identity2), InspectHandle::tree(proxy, Some("test")));

        assert_eq!(
            2,
            data_repo
                .inner
                .read()
                .fetch_inspect_data(&None, StaticHierarchyAllowlist::new_disabled())
                .len()
        );

        let selectors = Some(vec![
            selectors::parse_selector::<FastError>("a/b/foo:root").expect("parse selector"),
        ]);
        assert_eq!(
            1,
            data_repo
                .inner
                .read()
                .fetch_inspect_data(&selectors, StaticHierarchyAllowlist::new_disabled())
                .len()
        );

        let selectors = Some(vec![
            selectors::parse_selector::<FastError>("a/b/f*:root").expect("parse selector"),
        ]);
        assert_eq!(
            2,
            data_repo
                .inner
                .read()
                .fetch_inspect_data(&selectors, StaticHierarchyAllowlist::new_disabled())
                .len()
        );

        let selectors =
            Some(vec![selectors::parse_selector::<FastError>("foo:root").expect("parse selector")]);
        assert_eq!(
            0,
            data_repo
                .inner
                .read()
                .fetch_inspect_data(&selectors, StaticHierarchyAllowlist::new_disabled())
                .len()
        );
    }

    #[fuchsia::test]
    async fn repo_removes_escrowed_data_on_token_peer_closed() {
        let repo = Arc::new(InspectRepository::new(vec![], fasync::Scope::new()));
        let moniker = ExtendedMoniker::parse_str("a/b/foo").unwrap();
        let identity = Arc::new(ComponentIdentity::new(moniker, TEST_URL));

        let inspector = Inspector::default();
        let (ep0, ep1) = zx::EventPair::create();
        repo.escrow_handle(
            Arc::clone(&identity),
            inspector.duplicate_vmo_with_rights(*ESCROW_TEST_RIGHTS).unwrap(),
            finspect::EscrowToken { token: ep1 },
            Some("escrow"),
            None,
        );
        {
            let guard = repo.inner.read();
            let container = guard.get(&identity);
            assert!(container.is_some());
            let mut handles = container.unwrap().handles();
            assert_eq!(handles.len(), 1);
            assert_matches!(handles.next().unwrap(), InspectHandle::Escrow { .. });
        }
        drop(ep0);
        while repo.inner.read().get(&identity).is_some() {
            fasync::Timer::new(fasync::MonotonicInstant::after(
                zx::MonotonicDuration::from_millis(100_i64),
            ))
            .await;
        }
    }

    #[fuchsia::test]
    async fn repo_overwrites_tree_on_escrow() {
        let repo = Arc::new(InspectRepository::new(vec![], fasync::Scope::new()));
        let moniker = ExtendedMoniker::parse_str("a/b/foo").unwrap();
        let identity = Arc::new(ComponentIdentity::new(moniker, TEST_URL));
        let (proxy, server_end) = fidl::endpoints::create_proxy::<finspect::TreeMarker>();
        let koid = proxy.as_channel().as_handle_ref().koid().unwrap();

        repo.add_inspect_handle(Arc::clone(&identity), InspectHandle::tree(proxy, Some("tree")));
        {
            let guard = repo.inner.read();
            let mut handles = guard.get(&identity).unwrap().handles();
            assert_eq!(handles.len(), 1);
            assert_matches!(handles.next().unwrap(), InspectHandle::Tree { .. });
        }

        let inspector = Inspector::default();
        let (_ep0, ep1) = zx::EventPair::create();
        repo.escrow_handle(
            Arc::clone(&identity),
            inspector.duplicate_vmo_with_rights(*ESCROW_TEST_RIGHTS).unwrap(),
            finspect::EscrowToken { token: ep1 },
            Some("escrow"),
            Some(koid),
        );
        {
            let guard = repo.inner.read();
            let mut handles = guard.get(&identity).unwrap().handles();
            assert_eq!(handles.len(), 1);
            assert_matches!(handles.next().unwrap(), InspectHandle::Escrow { .. });
        }
        fasync::Channel::from_channel(server_end.into_channel()).on_closed().await.unwrap();
    }

    #[fuchsia::test]
    async fn repo_fetch_escrow_removes_data() {
        let repo = Arc::new(InspectRepository::new(vec![], fasync::Scope::new()));
        let moniker = ExtendedMoniker::parse_str("a/b/foo").unwrap();
        let identity = Arc::new(ComponentIdentity::new(moniker, TEST_URL));

        let inspector = Inspector::default();
        inspector.root().record_int("foo", 3);
        let (ep0, ep1) = zx::EventPair::create();
        repo.escrow_handle(
            Arc::clone(&identity),
            inspector.duplicate_vmo_with_rights(*ESCROW_TEST_RIGHTS).unwrap(),
            finspect::EscrowToken { token: ep1 },
            Some("escrow"),
            None,
        );
        {
            let guard = repo.inner.read();
            let container = guard.get(&identity);
            assert!(container.is_some());
            assert_eq!(container.unwrap().handles().len(), 1);
        }

        let vmo =
            repo.fetch_escrow(Arc::clone(&identity), finspect::EscrowToken { token: ep0 }, None);
        assert!(vmo.is_some());
        let vmo = vmo.unwrap();
        assert_eq!(vmo.get_name().unwrap(), INSPECT_ESCROW_NAME);
        let inspector_loaded = Inspector::new(InspectorConfig::default().vmo(vmo));
        assert_data_tree!(inspector_loaded, root: {
            foo: 3i64,
        });

        let guard = repo.inner.read();
        let container = guard.get(&identity);
        assert!(container.is_some());
        assert_eq!(container.unwrap().handles().len(), 0);
    }

    #[fuchsia::test]
    fn repo_fetch_escrow_with_tree_returns_data_keeps_tree() {
        let _exec = fuchsia_async::LocalExecutor::default();
        let repo = Arc::new(InspectRepository::new(vec![], fasync::Scope::new()));
        let moniker = ExtendedMoniker::parse_str("a/b/foo").unwrap();
        let identity = Arc::new(ComponentIdentity::new(moniker, TEST_URL));

        let inspector = Inspector::default();
        inspector.root().record_int("foo", 3);
        let (ep0, ep1) = zx::EventPair::create();
        repo.escrow_handle(
            Arc::clone(&identity),
            inspector.duplicate_vmo_with_rights(*ESCROW_TEST_RIGHTS).unwrap(),
            finspect::EscrowToken { token: ep1 },
            Some("escrow"),
            None,
        );
        {
            let guard = repo.inner.read();
            let container = guard.get(&identity);
            assert!(container.is_some());
            let mut handles = container.unwrap().handles();
            assert_eq!(handles.len(), 1);
            assert_matches!(handles.next().unwrap(), InspectHandle::Escrow { .. });
        }

        let (client_end, server_end) = fidl::endpoints::create_endpoints::<finspect::TreeMarker>();
        let vmo = repo.fetch_escrow(
            Arc::clone(&identity),
            finspect::EscrowToken { token: ep0 },
            Some(client_end),
        );
        assert!(vmo.is_some());
        {
            let guard = repo.inner.read();
            let mut handles = guard.get(&identity).unwrap().handles();
            assert_eq!(handles.len(), 1);
            assert_matches!(handles.next().unwrap(), InspectHandle::Tree { .. });
        }
        assert!(!fasync::Channel::from_channel(server_end.into_channel()).is_closed());
    }
}
