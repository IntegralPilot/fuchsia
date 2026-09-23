use fidl::endpoints;
use fidl::endpoints::{ClientEnd, ServerEnd};
use fidl::handle::Signals;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fprocess;
use fuchsia_async as fasync;
use fuchsia_component_test::{RealmBuilder, RealmBuilderParams};
use fuchsia_criterion::{FuchsiaCriterion, criterion};
use fuchsia_runtime::{HandleInfo, HandleType};
use fuchsia_sync::Mutex;
use std::sync::Arc;
use std::time::Duration;

#[fuchsia::main]
fn main() {
    log::info!("Started");

    // Initialize benchmark.
    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut criterion::Criterion = &mut c;
    *internal_c = std::mem::take(internal_c)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(1))
        .sample_size(20);

    let executor = Arc::new(Mutex::new(fasync::LocalExecutor::default()));
    let realm = executor.lock().run_singlethreaded(async move {
        // One instance of nested component manager shared by all tests.
        let builder = RealmBuilder::with_params(
            RealmBuilderParams::new().from_relative_url("#meta/root_component.cm"),
        )
        .await
        .unwrap();
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap()
    });
    let realm_proxy: fcomponent::RealmProxy = realm
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("could not connect to Realm service");

    let mut group = c.benchmark_group("fuchsia.component.lifecycle");

    let e = executor.clone();
    let p = realm_proxy.clone();
    let _ = group.bench_function("ElfComponent/start", move |b| {
        b.iter_with_setup(
            || {
                ElfComponentLaunchTest::new(
                    e.clone(),
                    &p,
                    "#meta/without_log.cm",
                    SetupMode::Resolved,
                )
            },
            |test| test.run(),
        );
    });
    let e = executor.clone();
    let p = realm_proxy.clone();
    let _ = group.bench_function("ElfComponent/start_and_resolve", move |b| {
        b.iter_with_setup(
            || {
                ElfComponentLaunchTest::new(
                    e.clone(),
                    &p,
                    "fuchsia-pkg://fuchsia.com/component_lifecycle_benchmark#meta/without_log.cm",
                    SetupMode::Unresolved,
                )
            },
            |test| test.run(),
        );
    });
    let e = executor.clone();
    let p = realm_proxy.clone();
    let _ = group.bench_function("ElfComponent/start_with_log", move |b| {
        b.iter_with_setup(
            || ElfComponentLaunchTest::new(e.clone(), &p, "#meta/with_log.cm", SetupMode::Resolved),
            |test| test.run(),
        );
    });
    let e = executor.clone();
    let p = realm_proxy.clone();
    let _ = group.bench_function("ElfComponent/start_with_log_and_stdout", move |b| {
        b.iter_with_setup(
            || {
                ElfComponentLaunchTest::new(
                    e.clone(),
                    &p,
                    "#meta/with_log_and_stdout.cm",
                    SetupMode::Resolved,
                )
            },
            |test| test.run(),
        );
    });

    group.finish();
}

struct ElfComponentLaunchTest {
    executor: Option<Arc<Mutex<fasync::LocalExecutor>>>,
    controller: fcomponent::ControllerProxy,
    ep1: zx::EventPair,
    ep2: zx::EventPair,
    _execution_client: ClientEnd<fcomponent::ExecutionControllerMarker>,
    execution_server: ServerEnd<fcomponent::ExecutionControllerMarker>,
}

enum SetupMode {
    Unresolved,
    Resolved,
}

impl ElfComponentLaunchTest {
    fn new(
        executor: Arc<Mutex<fasync::LocalExecutor>>,
        realm: &fcomponent::RealmProxy,
        url: &str,
        mode: SetupMode,
    ) -> Self {
        let controller = executor.lock().run_singlethreaded(Self::setup(realm, url, mode));
        let (ep1, ep2) = zx::EventPair::create();
        let (_execution_client, execution_server) =
            endpoints::create_endpoints::<fcomponent::ExecutionControllerMarker>();
        Self { executor: Some(executor), controller, ep1, ep2, _execution_client, execution_server }
    }

    async fn setup(
        realm: &fcomponent::RealmProxy,
        url: &str,
        mode: SetupMode,
    ) -> fcomponent::ControllerProxy {
        let (controller, controller_server) =
            endpoints::create_proxy::<fcomponent::ControllerMarker>();
        let collection_ref = fdecl::CollectionRef { name: "coll".into() };
        let id: u64 = rand::random();
        let child_name = format!("auto-{:x}", id);
        let child_decl = fdecl::Child {
            name: Some(child_name.clone().into()),
            url: Some(url.into()),
            startup: Some(fdecl::StartupMode::Lazy),
            ..Default::default()
        };
        let args = fcomponent::CreateChildArgs {
            controller: Some(controller_server),
            ..Default::default()
        };
        realm
            .create_child(&collection_ref, &child_decl, args)
            .await
            .unwrap_or_else(|e| panic!("create_child failed: {:?}", e))
            .unwrap_or_else(|e| panic!("failed to create child: {:?}", e));
        match mode {
            SetupMode::Unresolved => {}
            SetupMode::Resolved => {
                let child =
                    fdecl::ChildRef { name: child_name.into(), collection: Some("coll".into()) };
                let (_dir, server) = endpoints::create_proxy::<fio::DirectoryMarker>();
                realm
                    .open_exposed_dir(&child, server)
                    .await
                    .unwrap_or_else(|e| panic!("open_exposed_dir failed: {:?}", e))
                    .unwrap_or_else(|e| panic!("failed to open exposed dir of child: {:?}", e));
            }
        }
        controller
    }

    fn run(mut self) {
        let executor = self.executor.take().unwrap();
        executor.lock().run_singlethreaded(async move { self.do_run().await });
    }

    async fn do_run(self) {
        // Measure from component start to process exit. We can accomplish this by waiting
        // for PEER_CLOSED on a handle passed to the component.
        //
        // We could listen for the component's stop lifecycle event instead, but that would
        // be less accurate because it takes time for the signal to propagate from the process,
        // to the runner, to component management.
        let hinfo = fprocess::HandleInfo {
            handle: self.ep2.into_handle(),
            id: HandleInfo::new(HandleType::User0, 0).as_raw(),
        };
        let args = fcomponent::StartChildArgs {
            numbered_handles: Some(vec![hinfo]),
            ..Default::default()
        };
        self.controller
            .start(args, self.execution_server)
            .await
            .unwrap_or_else(|e| panic!("start failed: {:?}", e))
            .unwrap_or_else(|e| panic!("failed to start child: {:?}", e));
        fasync::OnSignals::new(self.ep1, Signals::OBJECT_PEER_CLOSED).await.unwrap();
    }
}
