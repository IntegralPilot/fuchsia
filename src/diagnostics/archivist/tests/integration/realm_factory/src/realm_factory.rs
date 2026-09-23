// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::*;
use cm_rust::{CapabilityDecl, DictionaryDecl};
use diagnostics_data::Severity;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_archivist_test::*;
use fidl_fuchsia_boot as fboot;
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_diagnostics_host as fdiagnostics_host;
use fidl_fuchsia_inspect as finspect;
use fidl_fuchsia_inspect::InspectSinkMarker;
use fidl_fuchsia_logger as flogger;
use fidl_fuchsia_logger::LogSinkMarker;
use fidl_fuchsia_tracing_provider as ftracing;
use flyweights::FlyStr;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, RealmInstance, Ref, Route,
};
use log::warn;

const ARCHIVIST_URL: &str = "#meta/archivist.cm";
const PUPPET_URL: &str = "puppet#meta/puppet.cm";
const STOP_WATCHER_URL: &str = "stop_watcher#meta/stop_watcher.cm";

#[derive(Default)]
pub(crate) struct ArchivistRealmFactory;

impl ArchivistRealmFactory {
    pub async fn create_realm(&mut self, options: RealmOptions) -> Result<RealmInstance, Error> {
        let mut params = RealmBuilderParams::new();
        if let Some(realm_name) = options.realm_name {
            params = params.realm_name(realm_name);
        }
        let builder = RealmBuilder::with_params(params).await?;
        let config = options.archivist_config.unwrap_or_default();

        // The following configurations are tweakble.
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.diagnostics.EnableKlog".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(
                    config.enable_klog.unwrap_or(false),
                )),
            }))
            .await?;
        if let Some(logs_max_cached_original_bytes) = config.logs_max_cached_original_bytes {
            builder
                .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                    name: "fuchsia.diagnostics.LogsMaxCachedOriginalBytes".parse()?,
                    value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Uint64(
                        logs_max_cached_original_bytes,
                    )),
                }))
                .await?;
        }
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.diagnostics.PipelinesPath".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::String(
                    config
                        .pipelines_path
                        .unwrap_or_else(|| "/pkg/data/config/pipelines/default".into())
                        .into(),
                )),
            }))
            .await?;

        let initial_interests = config
            .initial_interests
            .map(|interests| {
                interests
                    .into_iter()
                    .filter_map(|interest| {
                        let (Some(moniker), Some(log_severity)) =
                            (&interest.moniker, &interest.log_severity)
                        else {
                            warn!(
                                interest:?;
                                "Expected both moniker and severity for the intial interest"
                            );
                            return None;
                        };
                        let severity = Severity::from(*log_severity);
                        Some(format!("{moniker}:{severity}"))
                    })
                    .map(FlyStr::new)
                    .collect::<Box<[FlyStr]>>()
            })
            .unwrap_or_default();
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.diagnostics.ComponentInitialInterests".parse()?,
                value: cm_rust::ConfigValue::Vector(cm_rust::ConfigVectorValue::StringVector(
                    initial_interests,
                )),
            }))
            .await?;

        // This child test realm allows us to downscope the event stream offered
        // to archivist to the #test subtree. We need this because it's not possible
        // to downscope an event stream to the ref "self". See
        // https://fxbug.dev/42082439 for more information.
        let test_realm = builder.add_child_realm("test", ChildOptions::new()).await?;
        let archivist =
            test_realm.add_child("archivist", ARCHIVIST_URL, ChildOptions::new()).await?;
        // LINT.IfChange
        let parent_to_archivist = Route::new()
            .capability(Capability::protocol::<flogger::LogSinkMarker>())
            .capability(Capability::protocol::<ftracing::RegistryMarker>().optional())
            .capability(Capability::protocol::<fboot::ReadOnlyLogMarker>());
        // LINT.ThenChange(//src/diagnostics/archivist/testing/realm-factory/meta/realm-factory.cml)
        let archivist_to_parent = Route::new()
            .capability(Capability::dictionary("diagnostics-accessors"))
            .capability(Capability::protocol::<fdiagnostics::LogSettingsMarker>())
            .capability(Capability::protocol::<fdiagnostics::LogStreamMarker>())
            .capability(Capability::protocol::<flogger::LogSinkMarker>())
            .capability(Capability::protocol::<finspect::InspectSinkMarker>())
            .capability(Capability::protocol::<flogger::LogMarker>());

        let mut self_to_archivist = Route::new()
            .capability(Capability::configuration("fuchsia.diagnostics.ComponentInitialInterests"))
            .capability(Capability::configuration("fuchsia.diagnostics.EnableKlog"))
            .capability(Capability::configuration("fuchsia.diagnostics.PipelinesPath"));

        // The following config capabilities are routed from "void" to archivist since we use the
        // package default config values for them.
        let mut void_to_archivist = Route::new()
            .capability(Capability::configuration("fuchsia.diagnostics.BindServices"))
            .capability(Capability::configuration(
                "fuchsia.diagnostics.MaximumConcurrentSnapshotsPerReader",
            ))
            .capability(Capability::configuration("fuchsia.diagnostics.AllowSerialLogs"))
            .capability(Capability::configuration("fuchsia.diagnostics.DenySerialLogs"));

        let logs_max_cached_original_bytes_config_capability =
            Capability::configuration("fuchsia.diagnostics.LogsMaxCachedOriginalBytes");
        if config.logs_max_cached_original_bytes.is_none() {
            void_to_archivist =
                void_to_archivist.capability(logs_max_cached_original_bytes_config_capability);
        } else {
            self_to_archivist =
                self_to_archivist.capability(logs_max_cached_original_bytes_config_capability);
        }

        test_realm.add_route(void_to_archivist.from(Ref::void()).to(&archivist)).await?;

        builder.add_route(parent_to_archivist.clone().from(Ref::parent()).to(&test_realm)).await?;
        builder.add_route(self_to_archivist.clone().from(Ref::self_()).to(&test_realm)).await?;
        test_realm.add_route(parent_to_archivist.from(Ref::parent()).to(&archivist)).await?;
        test_realm.add_route(self_to_archivist.from(Ref::parent()).to(&archivist)).await?;

        test_realm
            .add_route(archivist_to_parent.clone().from(&archivist).to(Ref::parent()))
            .await?;
        builder.add_route(archivist_to_parent.from(&test_realm).to(Ref::parent())).await?;

        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::event_stream("capability_requested").with_scope(&test_realm),
                    )
                    .capability(Capability::event_stream("stopped").with_scope(&test_realm))
                    .from(Ref::parent())
                    .to(&test_realm),
            )
            .await?;
        test_realm
            .add_route(
                Route::new()
                    .capability(Capability::event_stream("capability_requested"))
                    .from(Ref::parent())
                    .to(&archivist),
            )
            .await?;

        // NOTE: needed for CTF. We can remove this when we stop supporting F25.
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fdiagnostics::ArchiveAccessorMarker>())
                    .capability(Capability::protocol::<fdiagnostics_host::ArchiveAccessorMarker>())
                    .capability(
                        Capability::protocol_by_name(format!(
                            "{}.feedback",
                            fdiagnostics::ArchiveAccessorMarker::PROTOCOL_NAME
                        ))
                        .as_("fuchsia.diagnostics.FeedbackArchiveAccessor"),
                    )
                    .capability(
                        Capability::protocol_by_name(format!(
                            "{}.lowpan",
                            fdiagnostics::ArchiveAccessorMarker::PROTOCOL_NAME
                        ))
                        .as_("fuchsia.diagnostics.LoWPANArchiveAccessor"),
                    )
                    .from(Ref::dictionary(&test_realm, "diagnostics-accessors"))
                    .to(Ref::parent()),
            )
            .await?;

        // Install the stop watcher component.
        let stop_watcher =
            test_realm.add_child("stop-watcher", STOP_WATCHER_URL, ChildOptions::new()).await?;
        test_realm
            .add_route(
                Route::new()
                    .capability(Capability::event_stream("stopped"))
                    .from(Ref::parent())
                    .to(&stop_watcher),
            )
            .await?;
        test_realm
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<StopWatcherMarker>())
                    .from(&stop_watcher)
                    .to(Ref::parent()),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<StopWatcherMarker>())
                    .from(&test_realm)
                    .to(Ref::parent()),
            )
            .await?;

        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::protocol_by_name("fuchsia.debugdata.Publisher").optional(),
                    )
                    .from(Ref::dictionary(Ref::parent(), "diagnostics"))
                    .to(&test_realm),
            )
            .await?;
        test_realm
            .add_capability(CapabilityDecl::Dictionary(DictionaryDecl {
                name: "diagnostics".parse().unwrap(),
                source_path: None,
            }))
            .await?;
        test_realm
            .add_route(
                Route::new()
                    .capability(
                        Capability::protocol_by_name("fuchsia.debugdata.Publisher").optional(),
                    )
                    .from(Ref::parent())
                    .to(Ref::dictionary(Ref::self_(), "diagnostics")),
            )
            .await?;
        test_realm
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<LogSinkMarker>())
                    .capability(Capability::protocol::<InspectSinkMarker>())
                    .from(&archivist)
                    .to(Ref::dictionary(Ref::self_(), "diagnostics")),
            )
            .await?;

        // Add the puppet components.
        if let Some(puppet_decls) = options.puppets {
            for decl in puppet_decls {
                let name = decl.name.clone().expect("puppet must have a name");
                let puppet = test_realm.add_child(&name, PUPPET_URL, ChildOptions::new()).await?;

                test_realm
                    .add_route(
                        Route::new()
                            .capability(Capability::protocol::<flogger::LogSinkMarker>())
                            .capability(Capability::protocol::<finspect::InspectSinkMarker>())
                            .from(&archivist)
                            .to(&puppet),
                    )
                    .await?;

                test_realm
                    .add_route(
                        Route::new()
                            .capability(Capability::dictionary("diagnostics"))
                            .from(Ref::self_())
                            .to(&puppet),
                    )
                    .await?;

                test_realm
                    .add_route(
                        Route::new()
                            .capability(
                                Capability::protocol::<PuppetMarker>()
                                    .as_(unique_puppet_protocol_name(&decl)),
                            )
                            .capability(
                                Capability::protocol_by_name(format!(
                                    "{}.nested_one",
                                    InspectPuppetMarker::PROTOCOL_NAME
                                ))
                                .as_(
                                    unique_puppet_child_inspect_protocol_name(&decl, "nested_one"),
                                ),
                            )
                            .capability(
                                Capability::protocol_by_name(format!(
                                    "{}.nested_two",
                                    InspectPuppetMarker::PROTOCOL_NAME
                                ))
                                .as_(
                                    unique_puppet_child_inspect_protocol_name(&decl, "nested_two"),
                                ),
                            )
                            .from(&puppet)
                            .to(Ref::parent()),
                    )
                    .await?;

                builder
                    .add_route(
                        Route::new()
                            .capability(Capability::protocol_by_name(unique_puppet_protocol_name(
                                &decl,
                            )))
                            .capability(Capability::protocol_by_name(
                                unique_puppet_child_inspect_protocol_name(&decl, "nested_one"),
                            ))
                            .capability(Capability::protocol_by_name(
                                unique_puppet_child_inspect_protocol_name(&decl, "nested_two"),
                            ))
                            .from(&test_realm)
                            .to(Ref::parent()),
                    )
                    .await?;
            }
        }
        Ok(builder.build().await?)
    }
}

fn unique_puppet_protocol_name(decl: &PuppetDecl) -> String {
    let name = decl.name.as_ref().unwrap();
    format!("{}.{}", PuppetMarker::PROTOCOL_NAME, name)
}

fn unique_puppet_child_inspect_protocol_name(decl: &PuppetDecl, nested_name: &str) -> String {
    let name = decl.name.as_ref().unwrap();
    format!("{}.{name}.{nested_name}", InspectPuppetMarker::PROTOCOL_NAME)
}
