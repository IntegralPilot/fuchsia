// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mocks;
use anyhow::*;
use cm_rust::{CapabilityDecl, DictionaryDecl};
use component_id_index::Index;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_hardware_power_statecontrol as fpower;
use fidl_fuchsia_inspect as finspect;
use fidl_fuchsia_logger as flogger;
use fidl_fuchsia_metrics as fmetrics;
use fidl_fuchsia_metrics_test as fmetrics_test;
use fidl_fuchsia_mockrebootcontroller as fmockrebootcontroller;
use fidl_fuchsia_samplertestcontroller as fsamplertestcontroller;
use fidl_test_sampler as ftest;
use flyweights::FlyStr;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmInstance, Ref, Route,
};
use futures::StreamExt;
use futures::channel::mpsc;
use futures::lock::Mutex;
use sampler_config::assembly::{ComponentIdInfoList, ProjectTemplate};
use std::sync::Arc;

const FAKE_COBALT_URL: &str = "#meta/fake_cobalt.cm";
const SINGLE_COUNTER_URL: &str = "#meta/single_counter_test_component.cm";
const SAMPLER_URL: &str = "#meta/sampler.cm";
const ARCHIVIST_URL: &str = "#meta/archivist-for-embedding.cm";
const SAMPLER_BINDER_ALIAS: &str = "fuchsia.component.SamplerBinder";
const PROJECT_CONFIGS_CAPABILITY: &str = "fuchsia.diagnostics.sampler.ProjectConfigs";

fn load_configs(index: Index) -> Vec<String> {
    let mut project_configs = vec![
        include_str!("../configs/reboot_required_config.json").to_string(),
        include_str!("../configs/test_config.json").to_string(),
    ];
    let mut components: ComponentIdInfoList =
        serde_json5::from_str(include_str!("../configs/components.json5")).unwrap();
    components.add_instance_ids(&index);
    let fire_templates = [
        include_str!("../configs/fire_1.json5"),
        include_str!("../configs/fire_2.json5"),
        include_str!("../configs/fire_3.json5"),
    ]
    .into_iter()
    .map(|s| serde_json5::from_str::<ProjectTemplate>(s).unwrap());
    for fire_template in fire_templates {
        let config = fire_template.expand(&components).expect("failed to expand fire template");
        project_configs.push(serde_json5::to_string(&config).unwrap());
    }
    project_configs
}

pub async fn create_realm(options: ftest::RealmOptions) -> Result<RealmInstance, Error> {
    let sampler_component_name = options.sampler_component_name.as_deref().unwrap_or("sampler");
    let single_counter_name = options.single_counter_name.as_deref().unwrap_or("single_counter");
    let fake_cobalt_name = options.fake_cobalt_name.as_deref().unwrap_or("fake_cobalt");
    let test_archivist_name =
        options.test_archivist_name.as_deref().unwrap_or("test_case_archivist");
    let builder = RealmBuilder::new().await?;
    let mocks_server = builder
        .add_local_child(
            "mocks-server",
            move |handles| Box::pin(serve_mocks(handles)),
            ChildOptions::new(),
        )
        .await?;
    let wrapper_realm = builder.add_child_realm("wrapper", ChildOptions::new()).await?;
    let fake_cobalt =
        wrapper_realm.add_child(fake_cobalt_name, FAKE_COBALT_URL, ChildOptions::new()).await?;
    let single_counter = wrapper_realm
        .add_child(single_counter_name, SINGLE_COUNTER_URL, ChildOptions::new())
        .await?;
    let sampler =
        wrapper_realm.add_child(sampler_component_name, SAMPLER_URL, ChildOptions::new()).await?;
    let test_case_archivist =
        wrapper_realm.add_child(test_archivist_name, ARCHIVIST_URL, ChildOptions::new()).await?;

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: PROJECT_CONFIGS_CAPABILITY.parse()?,
            value: cm_rust::ConfigValue::Vector(cm_rust::ConfigVectorValue::StringVector(
                load_configs(mocks::fake_index())
                    .into_iter()
                    .map(FlyStr::new)
                    .collect::<Box<[FlyStr]>>(),
            )),
        }))
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration(PROJECT_CONFIGS_CAPABILITY))
                .from(Ref::self_())
                .to(&wrapper_realm),
        )
        .await?;
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::configuration(PROJECT_CONFIGS_CAPABILITY))
                .from(Ref::parent())
                .to(&sampler),
        )
        .await?;

    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fmetrics_test::MetricEventLoggerQuerierMarker>())
                .from(&fake_cobalt)
                .to(Ref::parent()),
        )
        .await?;
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<
                    fsamplertestcontroller::SamplerTestControllerMarker,
                >())
                .from(&single_counter)
                .to(Ref::parent()),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fmetrics_test::MetricEventLoggerQuerierMarker>())
                .capability(Capability::protocol::<
                    fsamplertestcontroller::SamplerTestControllerMarker,
                >())
                .from(&wrapper_realm)
                .to(Ref::parent()),
        )
        .await?;

    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fmetrics::MetricEventLoggerFactoryMarker>())
                .from(&fake_cobalt)
                .to(&sampler),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fpower::ShutdownWatcherRegisterMarker>())
                .from(&mocks_server)
                .to(&wrapper_realm),
        )
        .await?;
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fpower::ShutdownWatcherRegisterMarker>())
                .from(Ref::parent())
                .to(&sampler),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(
                    Capability::protocol::<fmockrebootcontroller::MockRebootControllerMarker>(),
                )
                .from(&mocks_server)
                .to(Ref::parent()),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::dictionary("diagnostics"))
                .from(Ref::parent())
                .to(&wrapper_realm),
        )
        .await?;

    wrapper_realm
        .add_capability(CapabilityDecl::Dictionary(DictionaryDecl {
            name: "diagnostics".parse().unwrap(),
            source_path: None,
        }))
        .await?;

    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<finspect::InspectSinkMarker>())
                .from(&test_case_archivist)
                .to(Ref::dictionary(Ref::self_(), "diagnostics")),
        )
        .await?;

    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<flogger::LogSinkMarker>())
                .from(Ref::dictionary(Ref::parent(), "diagnostics"))
                .to(Ref::dictionary(Ref::self_(), "diagnostics")),
        )
        .await?;

    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::dictionary("diagnostics"))
                .from(Ref::self_())
                .to(&fake_cobalt)
                .to(&sampler)
                .to(&single_counter),
        )
        .await?;

    // TODO(https://fxbug.dev/42156520): refactor these tests to use the single test archivist and remove
    // this archivist. We can also remove the `wrapper` realm when this is done. The
    // Log protocols routed here would be routed from AboveRoot instead. To
    // do so, uncomment the following routes and delete all the routes after this comment
    // involving "wrapper/test_case_archivist":
    // .add_route(RouteBuilder::protocol("fuchsia.logger.Log")
    //     .source(RouteEndpoint::AboveRoot)
    //     .targets(vec![RouteEndpoint::component("wrapper/sampler")])
    // }).await?
    builder
        .add_route(
            Route::new()
                .capability(
                    Capability::event_stream("capability_requested").with_scope(&wrapper_realm),
                )
                .from(Ref::parent())
                .to(&wrapper_realm),
        )
        .await?;
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<flogger::LogMarker>())
                .from(&test_case_archivist)
                .to(&sampler),
        )
        .await?;
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fdiagnostics::SampleMarker>())
                .from(Ref::dictionary(&test_case_archivist, "diagnostics-accessors"))
                .to(&sampler),
        )
        .await?;
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fdiagnostics::ArchiveAccessorMarker>())
                .from(Ref::dictionary(&test_case_archivist, "diagnostics-accessors"))
                .to(Ref::parent()),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fdiagnostics::ArchiveAccessorMarker>())
                .from(&wrapper_realm)
                .to(Ref::parent()),
        )
        .await?;
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::event_stream("capability_requested"))
                .from(Ref::parent())
                .to(&test_case_archivist),
        )
        .await?;

    wrapper_realm
        .add_route(
            Route::new()
                .capability(
                    Capability::protocol::<fcomponent::BinderMarker>().as_(SAMPLER_BINDER_ALIAS),
                )
                .from(&sampler)
                .to(Ref::parent()),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(SAMPLER_BINDER_ALIAS))
                .from(&wrapper_realm)
                .to(Ref::parent()),
        )
        .await?;

    let instance = builder.build().await?;

    Ok(instance)
}

async fn serve_mocks(handles: LocalComponentHandles) -> Result<(), Error> {
    let mut fs = ServiceFs::new();

    let (snd, rcv) = mpsc::channel(1);
    let rcv = Arc::new(Mutex::new(rcv));

    fs.dir("svc")
        .add_fidl_service(move |stream| {
            mocks::serve_shutdown_server(stream, snd.clone());
        })
        .add_fidl_service(move |stream| {
            mocks::serve_reboot_controller(stream, rcv.clone());
        });

    fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;
    Ok(())
}
