// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::sandbox_util::take_handle_as_stream;
use cm_rust::FidlIntoNative;
use fidl::endpoints::ProtocolMarker;
use fidl_fuchsia_component_decl as fcdecl;
use fidl_fuchsia_sys2 as fsys;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use log::warn;
use moniker::Moniker;
use routing::component_instance::ComponentInstanceInterface;

use crate::model::component::WeakComponentInstance;

pub fn serve(
    server_end: zx::Channel,
    _target: WeakComponentInstance,
    source: WeakComponentInstance,
) -> BoxFuture<'static, Result<(), anyhow::Error>> {
    async move {
        let mut stream = take_handle_as_stream::<fsys::ConfigOverrideMarker>(server_end);
        while let Some(Ok(request)) = stream.next().await {
            let result = match request {
                fsys::ConfigOverrideRequest::SetStructuredConfig { moniker, fields, responder } => {
                    let fields = fields.into_iter().map(FidlIntoNative::fidl_into_native).collect();
                    let result =
                        set_structured_config(&source, &moniker, fields).await;
                    responder.send(result)
                }
                fsys::ConfigOverrideRequest::UnsetStructuredConfig { moniker, responder } => {
                    let result = unset_structured_config(&source, &moniker).await;
                    responder.send(result)
                }
                fsys::ConfigOverrideRequest::_UnknownMethod { ordinal, method_type, .. } => {
                    warn!(
                        "{} received request for unknown method with ordinal {ordinal} and method type {method_type:?}",
                        fsys::ConfigOverrideMarker::DEBUG_NAME
                    );
                    Ok(())
                }
            };
            if let Err(error) = result {
                warn!(error:?; "Could not respond to ConfigOverride request");
                break;
            }
        }
        Ok(())
    }.boxed()
}

async fn set_structured_config(
    scope: &WeakComponentInstance,
    moniker: &str,
    fields: Vec<cm_rust::ConfigOverride>,
) -> Result<(), fsys::ConfigOverrideError> {
    // Construct the complete moniker using the scope moniker and the moniker string.
    let moniker = Moniker::try_from(moniker).map_err(|_| fsys::ConfigOverrideError::BadMoniker)?;
    let moniker = scope.moniker.concat(&moniker);
    let instance = scope
        .upgrade()
        .map_err(|_| fsys::ConfigOverrideError::InstanceNotFound)?
        .find_absolute(&moniker)
        .await
        .map_err(|_| fsys::ConfigOverrideError::InstanceNotFound)?;

    let state = instance.lock_state().await;

    let arc_config = state
        .get_resolved_state()
        .ok_or(fsys::ConfigOverrideError::InstanceNotResolved)?
        .config()
        .ok_or(fsys::ConfigOverrideError::NoConfig)?;

    let config: fcdecl::ResolvedConfig = (**arc_config).clone().into();

    for field in fields {
        // Verify a field with this key has been declared for the component.
        config
            .fields
            .iter()
            .find(|f| *f.key == *field.key)
            .ok_or(fsys::ConfigOverrideError::KeyNotFound)?;
        instance.context.add_config_developer_override(moniker.clone(), field).await;
    }
    Ok(())
}

async fn unset_structured_config(
    scope: &WeakComponentInstance,
    moniker: &str,
) -> Result<(), fsys::ConfigOverrideError> {
    let scope = scope.upgrade().map_err(|_| fsys::ConfigOverrideError::InstanceNotFound)?;
    if moniker.is_empty() {
        return Ok(scope.context.clear_config_developer_override(&scope.moniker).await);
    }

    // Construct the complete moniker using the scope moniker and the moniker string.
    let moniker = Moniker::try_from(moniker).map_err(|_| fsys::ConfigOverrideError::BadMoniker)?;
    let moniker = scope.moniker.concat(&moniker);

    // Verify that the instance specified by moniker exists.
    let _instance = scope
        .find_absolute(&moniker)
        .await
        .map_err(|_| fsys::ConfigOverrideError::InstanceNotFound)?;
    scope
        .context
        .remove_config_developer_override(&moniker)
        .await
        .map_err(|_e| fsys::ConfigOverrideError::NoConfig)
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::testing::test_helpers::{
        TestEnvironmentBuilder, config_override, lifecycle_controller, new_config_decl,
    };
    use cm_rust::{ConfigSingleValue, ConfigValue, NativeIntoFidl};
    use cm_rust_testing::*;

    #[fuchsia::test]
    async fn set_structured_config_test() {
        let (config, config_values, _checksum) = new_config_decl();
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("has_config").eager())
                    .child(ChildBuilder::new().name("no_config").eager())
                    .build(),
            ),
            ("has_config", ComponentDeclBuilder::new().config(config).build()),
            ("no_config", ComponentDeclBuilder::new().build()),
        ];

        let test_model_result = TestEnvironmentBuilder::new()
            .set_components(components)
            .set_config_values(vec![("meta/root.cvf", config_values)])
            .build()
            .await;
        let config_override_proxy = config_override(&test_model_result).await;
        let lifecycle_controller_proxy = lifecycle_controller(&test_model_result).await;

        lifecycle_controller_proxy.resolve_instance(".").await.unwrap().unwrap();
        lifecycle_controller_proxy.resolve_instance("./has_config").await.unwrap().unwrap();
        lifecycle_controller_proxy.resolve_instance("./no_config").await.unwrap().unwrap();

        assert_eq!(
            config_override_proxy.set_structured_config("", &[]).await.unwrap(),
            Err(fsys::ConfigOverrideError::BadMoniker)
        );

        assert_eq!(
            config_override_proxy.set_structured_config("./doesnotexist", &[]).await.unwrap(),
            Err(fsys::ConfigOverrideError::InstanceNotFound)
        );

        lifecycle_controller_proxy.unresolve_instance("./has_config").await.unwrap().unwrap();
        assert_eq!(
            config_override_proxy.set_structured_config("./has_config", &[]).await.unwrap(),
            Err(fsys::ConfigOverrideError::InstanceNotResolved)
        );
        lifecycle_controller_proxy.resolve_instance("./has_config").await.unwrap().unwrap();

        assert_eq!(
            config_override_proxy.set_structured_config("./no_config", &[]).await.unwrap(),
            Err(fsys::ConfigOverrideError::NoConfig),
        );

        assert_eq!(
            config_override_proxy
                .set_structured_config(
                    "./has_config",
                    &[cm_rust::ConfigOverride {
                        key: "bogus_key".into(),
                        value: ConfigValue::Single(ConfigSingleValue::Bool(true))
                    }
                    .native_into_fidl()]
                )
                .await
                .unwrap(),
            Err(fsys::ConfigOverrideError::KeyNotFound),
        );

        assert_eq!(
            config_override_proxy
                .set_structured_config(
                    "./has_config",
                    &[cm_rust::ConfigOverride {
                        key: "my_field".into(),
                        value: ConfigValue::Single(ConfigSingleValue::Bool(false))
                    }
                    .native_into_fidl()]
                )
                .await
                .unwrap(),
            Ok(())
        );
    }

    #[fuchsia::test]
    async fn unset_structured_config_test() {
        let (config, config_values, _checksum) = new_config_decl();
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new().child(ChildBuilder::new().name("a").eager()).build(),
            ),
            ("a", ComponentDeclBuilder::new().config(config).build()),
        ];

        let test_model_result = TestEnvironmentBuilder::new()
            .set_components(components)
            .set_config_values(vec![("meta/root.cvf", config_values)])
            .build()
            .await;
        let config_override_proxy = config_override(&test_model_result).await;
        let lifecycle_controller_proxy = lifecycle_controller(&test_model_result).await;

        lifecycle_controller_proxy.resolve_instance(".").await.unwrap().unwrap();
        lifecycle_controller_proxy.resolve_instance("./a").await.unwrap().unwrap();
        config_override_proxy
            .set_structured_config(
                "./a",
                &[cm_rust::ConfigOverride {
                    key: "my_field".into(),
                    value: ConfigValue::Single(ConfigSingleValue::Bool(false)),
                }
                .native_into_fidl()],
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            config_override_proxy.unset_structured_config("not:valid:moniker").await.unwrap(),
            Err(fsys::ConfigOverrideError::BadMoniker)
        );

        assert_eq!(
            config_override_proxy.unset_structured_config("./doesnotexist").await.unwrap(),
            Err(fsys::ConfigOverrideError::InstanceNotFound)
        );

        assert_eq!(config_override_proxy.unset_structured_config("./a").await.unwrap(), Ok(()));

        assert_eq!(
            config_override_proxy.unset_structured_config("./a").await.unwrap(),
            Err(fsys::ConfigOverrideError::NoConfig),
        );
    }
}
