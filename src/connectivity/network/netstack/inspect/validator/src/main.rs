// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_diagnostics_validate as validate;
use fidl_diagnostics_validate_deprecated as validate_deprecated;
use fuchsia_async::Scope;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::{Inspector, InspectorConfig};
use futures::{StreamExt, TryStreamExt};
use inspect_runtime::PublishOptions;

enum IncomingService {
    InspectPuppet(validate::InspectPuppetRequestStream),
}

#[fuchsia::main]
async fn main() {
    let mut fs = ServiceFs::new_local();
    let _ = fs.dir("svc").add_fidl_service(IncomingService::InspectPuppet);

    let _ = fs.take_and_serve_directory_handle();

    fs.for_each_concurrent(None, |IncomingService::InspectPuppet(stream)| async move {
        run_service(stream).await
    })
    .await
}

/// A local trait is needed to define conversions between the two FIDL protocols,
/// since those are both foreign.
trait LocalFrom<T> {
    fn local_from(src: T) -> Self;
}

impl LocalFrom<Option<validate_deprecated::DiffType>> for Option<validate::DiffType> {
    fn local_from(deprecated: Option<validate_deprecated::DiffType>) -> Self {
        deprecated.map(|deprecated| match deprecated {
            validate_deprecated::DiffType::Full => validate::DiffType::Full,
            validate_deprecated::DiffType::Diff => validate::DiffType::Diff,
            validate_deprecated::DiffType::Both => validate::DiffType::Both,
        })
    }
}

impl LocalFrom<validate_deprecated::Options> for validate::Options {
    fn local_from(deprecated: validate_deprecated::Options) -> Self {
        let validate_deprecated::Options { has_runner_node, diff_type, .. } = deprecated;
        Self { has_runner_node, diff_type: LocalFrom::local_from(diff_type), ..Default::default() }
    }
}

impl LocalFrom<validate::InitializationParams> for validate_deprecated::InitializationParams {
    fn local_from(params: validate::InitializationParams) -> Self {
        let validate::InitializationParams { vmo_size, .. } = params;
        Self { vmo_size, ..Default::default() }
    }
}

impl LocalFrom<validate_deprecated::TestResult> for validate::TestResult {
    fn local_from(deprecated: validate_deprecated::TestResult) -> Self {
        match deprecated {
            validate_deprecated::TestResult::Ok => Self::Ok,
            validate_deprecated::TestResult::Unimplemented => Self::Unimplemented,
            validate_deprecated::TestResult::Failed => Self::Failed,
            validate_deprecated::TestResult::Illegal => Self::Illegal,
        }
    }
}

impl LocalFrom<validate::CreateNode> for validate_deprecated::CreateNode {
    fn local_from(cn: validate::CreateNode) -> Self {
        let validate::CreateNode { parent, id, name } = cn;
        Self { parent, id, name }
    }
}

impl LocalFrom<validate::DeleteNode> for validate_deprecated::DeleteNode {
    fn local_from(dn: validate::DeleteNode) -> Self {
        let validate::DeleteNode { id } = dn;
        Self { id }
    }
}

impl LocalFrom<validate::CreateNumericProperty>
    for Option<validate_deprecated::CreateNumericProperty>
{
    fn local_from(cnp: validate::CreateNumericProperty) -> Self {
        let validate::CreateNumericProperty { parent, id, name, value } = cnp;
        let value = match value {
            validate::Value::IntT(i) => validate_deprecated::Value::IntT(i),
            validate::Value::UintT(u) => validate_deprecated::Value::UintT(u),
            validate::Value::DoubleT(d) => validate_deprecated::Value::DoubleT(d),
            validate::Value::StringT(s) => validate_deprecated::Value::StringT(s),
            _ => return None,
        };

        Some(validate_deprecated::CreateNumericProperty { parent, id, name, value })
    }
}

impl LocalFrom<validate::CreateBytesProperty> for validate_deprecated::CreateBytesProperty {
    fn local_from(cbp: validate::CreateBytesProperty) -> Self {
        let validate::CreateBytesProperty { parent, id, name, value } = cbp;
        Self { parent, id, name, value }
    }
}

impl LocalFrom<validate::Action> for Option<validate_deprecated::Action> {
    fn local_from(action: validate::Action) -> Self {
        match action {
            validate::Action::CreateNode(cn) => {
                Some(validate_deprecated::Action::CreateNode(LocalFrom::local_from(cn)))
            }
            validate::Action::DeleteNode(dn) => {
                Some(validate_deprecated::Action::DeleteNode(LocalFrom::local_from(dn)))
            }
            validate::Action::CreateNumericProperty(cnp) => {
                <Option<validate_deprecated::CreateNumericProperty> as LocalFrom<
                    validate::CreateNumericProperty,
                >>::local_from(cnp)
                .map(validate_deprecated::Action::CreateNumericProperty)
            }
            validate::Action::CreateBytesProperty(cbp) => {
                Some(validate_deprecated::Action::CreateBytesProperty(LocalFrom::local_from(cbp)))
            }
            _ => None,
        }
    }
}

async fn run_service(mut incoming: validate::InspectPuppetRequestStream) {
    let Ok(go_puppet) = connect_to_protocol::<validate_deprecated::InspectPuppetMarker>() else {
        return;
    };

    let running_inspect_servers = Scope::new();

    while let Ok(Some(event)) = incoming.try_next().await {
        match event {
            validate::InspectPuppetRequest::Initialize { responder, params, .. } => {
                let (vmo, result) = go_puppet
                    .initialize(&LocalFrom::local_from(params))
                    .await
                    .expect("puppet did not respond");

                let _ = running_inspect_servers.spawn(
                    inspect_runtime::publish(
                        &Inspector::new(
                            InspectorConfig::default().vmo(
                                vmo.as_ref()
                                    .expect("vmo was returned from puppet-internal")
                                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                    .expect("vmo handle is able to be duplicated")
                                    .into(),
                            ),
                        ),
                        PublishOptions::default(),
                    )
                    .expect("Inspect was published"),
                );
                responder.send(vmo, LocalFrom::local_from(result)).expect("send failed")
            }
            validate::InspectPuppetRequest::GetConfig { responder, .. } => {
                let (name, options) = go_puppet.get_config().await.expect("puppet did not respond");
                responder.send(&name, LocalFrom::local_from(options)).expect("send failed");
            }
            validate::InspectPuppetRequest::InitializeTree { responder, .. } => {
                responder.send(None, validate::TestResult::Unimplemented).expect("send failed");
            }
            validate::InspectPuppetRequest::Publish { responder, .. } => {
                let result = go_puppet.publish().await.expect("puppet did not respond");
                responder.send(LocalFrom::local_from(result)).expect("send failed");
            }
            validate::InspectPuppetRequest::Act { responder, action, .. } => {
                if let Some(action) = LocalFrom::local_from(action) {
                    let result = go_puppet.act(&action).await.expect("puppet did not respond");
                    responder.send(LocalFrom::local_from(result)).expect("send failed");
                } else {
                    responder.send(validate::TestResult::Unimplemented).expect("send failed");
                }
            }
            validate::InspectPuppetRequest::ActLazy { responder, .. } => {
                responder.send(validate::TestResult::Unimplemented).expect("send failed");
            }
            validate::InspectPuppetRequest::ActLazyThreadLocal { responder, .. } => {
                responder.send(validate::TestResult::Unimplemented).expect("send failed");
            }
            validate::InspectPuppetRequest::_UnknownMethod { .. } => {}
        }
    }
}
