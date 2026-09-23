// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_sys2 as fsys;
use fuchsia_inspect::Property;
use futures::prelude::*;
use log::*;
use std::sync::Arc;

pub struct BootController {
    _inspect_node: fuchsia_inspect::Node,
    purge_timestamp: fuchsia_inspect::UintProperty,
    purge_fn: Box<dyn Fn() -> Result<(), ()> + Send + Sync + 'static>,
}

const INSPECT_KEY: &str = "last_memory_purge_timestamp";
impl BootController {
    pub fn new(inspect_node: fuchsia_inspect::Node) -> Arc<Self> {
        Self::new_inner(inspect_node, || scudo::mallopt(scudo::M_PURGE_ALL, 0))
    }

    fn new_inner(
        inspect_node: fuchsia_inspect::Node,
        purge_fn: impl Fn() -> Result<(), ()> + Send + Sync + 'static,
    ) -> Arc<Self> {
        let purge_timestamp = inspect_node.create_uint(INSPECT_KEY, 0);
        Arc::new(Self {
            _inspect_node: inspect_node,
            purge_fn: Box::new(purge_fn),
            purge_timestamp,
        })
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fsys::BootControllerRequestStream,
    ) -> Result<(), anyhow::Error> {
        while let Some(Ok(request)) = stream.next().await {
            match request {
                fsys::BootControllerRequest::Notify { responder } => {
                    match (self.purge_fn)() {
                        Ok(()) => {
                            info!("mallopt(M_PURGE_ALL)");
                            let ts = zx::BootInstant::get().into_nanos();
                            assert!(ts > 0, "zx::BootInstant > 0");
                            self.purge_timestamp.set(ts as u64);
                        }
                        Err(()) => warn!(
                            "mallopt(M_PURGE_ALL) failed. This is expected on sanitizer builds."
                        ),
                    }
                    let _ = responder.send();
                }
                unknown_request => warn!("BootController: unknown request: {unknown_request:?}"),
            }
        }
        Ok(())
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;

    use fidl::endpoints;
    use fuchsia_async as fasync;
    use fuchsia_inspect::DiagnosticsHierarchyGetter;
    use futures::channel::mpsc;

    async fn read_property(inspector: &fuchsia_inspect::Inspector) -> u64 {
        let hierarchy = inspector.get_diagnostics_hierarchy().await;
        let node = hierarchy.get_child("boot").unwrap();
        node.get_property(INSPECT_KEY).unwrap().uint().unwrap()
    }

    #[fuchsia::test]
    async fn purge_repeated() {
        let scope = fasync::Scope::new();

        let inspector = fuchsia_inspect::Inspector::default();
        let node = inspector.root().create_child("boot");
        let (tx, mut rx) = mpsc::unbounded();
        let s = scope.clone();
        let controller = BootController::new_inner(node, move || {
            let mut tx = tx.clone();
            s.spawn(async move {
                tx.send(()).await.unwrap();
            });
            Ok(())
        });

        let ts0 = read_property(&inspector).await;
        assert_eq!(ts0, 0);

        let (client, stream) = endpoints::create_proxy_and_stream::<fsys::BootControllerMarker>();
        scope.spawn(async move {
            controller.serve(stream).await.unwrap();
        });

        client.notify().await.unwrap();
        let () = rx.next().await.unwrap();
        let ts1 = read_property(&inspector).await;
        assert!(ts1 > 0, "{ts1} > 0");

        client.notify().await.unwrap();
        let () = rx.next().await.unwrap();
        let ts2 = read_property(&inspector).await;
        assert!(ts2 > ts1, "{ts2} > {ts1}");
    }

    #[fuchsia::test]
    async fn purge_with_error() {
        let scope = fasync::Scope::new();

        let inspector = fuchsia_inspect::Inspector::default();
        let node = inspector.root().create_child("boot");
        let (tx, mut rx) = mpsc::unbounded();
        let s = scope.clone();
        let controller = BootController::new_inner(node, move || {
            let mut tx = tx.clone();
            s.spawn(async move {
                tx.send(()).await.unwrap();
            });
            Err(())
        });

        let ts0 = read_property(&inspector).await;
        assert_eq!(ts0, 0);

        let (client, stream) = endpoints::create_proxy_and_stream::<fsys::BootControllerMarker>();
        scope.spawn(async move {
            controller.serve(stream).await.unwrap();
        });

        client.notify().await.unwrap();
        let () = rx.next().await.unwrap();
        let ts1 = read_property(&inspector).await;
        assert_eq!(ts1, 0);
    }
}
