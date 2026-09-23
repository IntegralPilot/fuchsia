// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::discovery_stream::{DiagnosticsResolver, NotifierMessage, SingleTargetResolver};
use discovery::query::TargetInfoQuery;
use discovery::{DiscoverySources, TargetHandle};
use ffx_config::EnvironmentContext;
use ffx_diagnostics::{Check, CheckFut, Notifier};
use ffx_diagnostics_formatting::TargetInfoQueryExt;
use futures::stream::StreamExt;
use std::marker::PhantomData;
use std::path::PathBuf;
use termio::Colors;

pub struct ResolveTarget<'a, N, R = SingleTargetResolver> {
    ctx: &'a EnvironmentContext,
    _resolver: PhantomData<R>,
    _notifier: PhantomData<N>,
}

impl<'a, N, R> ResolveTarget<'a, N, R>
where
    R: DiagnosticsResolver,
{
    pub fn new(ctx: &'a EnvironmentContext) -> Self {
        Self { ctx, _resolver: Default::default(), _notifier: Default::default() }
    }
}

// This may need some tweaking if looking at too many/too few sources for each query type.
fn sources_from_query(query: &TargetInfoQuery) -> DiscoverySources {
    match query {
        TargetInfoQuery::NodenameOrId(_) | TargetInfoQuery::First | TargetInfoQuery::Id(_) => {
            DiscoverySources::all()
        }
        TargetInfoQuery::VSock(_) => DiscoverySources::USB_FASTBOOT | DiscoverySources::EMULATOR,
        TargetInfoQuery::Usb(_) => DiscoverySources::USB_FASTBOOT,
        TargetInfoQuery::Addr(_) => {
            DiscoverySources::MDNS
                | DiscoverySources::FASTBOOT_FILE
                | DiscoverySources::EMULATOR
                | DiscoverySources::MANUAL
                | DiscoverySources::GCE
        }
    }
}

fn notify_for_discovery_sources<N>(
    ctx: &EnvironmentContext,
    sources: DiscoverySources,
    notifier: &mut N,
) -> anyhow::Result<()>
where
    N: Notifier + Sized,
{
    if sources.contains(DiscoverySources::EMULATOR) {
        // This isn't an option as it's intended to be part of the default config.
        let emu_instance_root: PathBuf = ctx.get(ffx_config::keys::EMU_INSTANCE_ROOT_DIR)?;
        notifier.info(format!("Searching for emulators at {}", emu_instance_root.display()))?;
    }
    if sources.contains(DiscoverySources::FASTBOOT_FILE) {
        let fastboot_file_path: Option<PathBuf> =
            ctx.get(ffx_config::keys::FASTBOOT_FILE_PATH).ok();
        // Note: there are no tests covering the `None` case because this is built into the default
        // config, and despite `no_environment` being set in the config we're still resolving this
        // to $HOME/.fastboot/devices even if nothing is set in our test environment.
        match fastboot_file_path {
            Some(p) => notifier.info(format!("Checking fastboot file at {}", p.display()))?,
            None => notifier.info(format!(
                "No fastboot file set in the config under {}",
                ffx_config::keys::FASTBOOT_FILE_PATH
            ))?,
        }
    }
    if sources.contains(DiscoverySources::GCE) {
        let gce_instance_root: Option<PathBuf> =
            ctx.get(ffx_config::keys::GCE_INSTANCE_ROOT_DIR).ok();
        if let Some(p) = gce_instance_root {
            notifier.info(format!("Searching for GCE instances at {}", p.display()))?;
        }
    }
    Ok(())
}

impl<N, R> Check for ResolveTarget<'_, N, R>
where
    N: Notifier + Sized,
    R: DiagnosticsResolver,
{
    type Input = TargetInfoQuery;
    type Output = TargetHandle;
    type Notifier = N;

    fn write_preamble(
        &self,
        input: &Self::Input,
        notifier: &mut Self::Notifier,
    ) -> anyhow::Result<()> {
        let sources = sources_from_query(&input);
        let sources_string =
            sources.iter_names().map(|(n, _)| n.to_owned()).collect::<Vec<_>>().join(", ");
        let nodename = match input {
            TargetInfoQuery::NodenameOrId(v) => Some(v),
            _ => None,
        };
        if let Some(nodename) = nodename {
            let safe_nodename = safe_string::TermSafe::from_str_escaped(nodename);
            notifier.info(format!(
                "Attempting to find device \"{safe_nodename}\" via {sources_string}..."
            ))
        } else {
            notifier.info(format!("Attempting to find device via {sources_string}..."))
        }
    }

    fn on_success(
        &self,
        output: &Self::Output,
        notifier: &mut Self::Notifier,
    ) -> anyhow::Result<()> {
        let colors = Colors::current();
        let state_str = ffx_diagnostics_formatting::format_target_state(&output.state);
        if let Some(name) = &output.node_name {
            let safe_name = safe_string::TermSafe::from_str_escaped(name);
            notifier.on_success(format!(
                "Device resolved to node: \"{}{}{}\" {state_str}",
                colors.green, safe_name, colors.reset
            ))
        } else {
            notifier.on_success(format!("Device resolved to be {state_str}"))
        }
    }

    fn check<'a>(
        &'a mut self,
        input: Self::Input,
        notifier: &'a mut Self::Notifier,
    ) -> CheckFut<'a, Self::Output> {
        // This step, on account of it being the most broad, will have the most potential ways to
        // fail, meaning that the solution space is quite wide. If this fails and there are no
        // devices found, then that means there are subsequently a great number of ways to resolve
        // the device. What we can perhaps do instead of discovering in parallel is to go serially,
        // with each failure to discover devices leading to a specific error.
        //
        // There should also not be certain errors if the device is formatted a given way. For
        // example: if the device is an IP address, we should not attempt to resolve the device via
        // mDNS, as we already have the IP address.
        Box::pin(async move {
            let sources = sources_from_query(&input);
            // There should be some kind of error here if the device resolves to an empty array.
            notify_for_discovery_sources(self.ctx, sources, notifier)?;
            let (notifier_sender, mut notification_stream) = futures::channel::mpsc::unbounded();
            let resolver = R::from_sources_and_notifier_sender(sources, notifier_sender);
            let (targets, ()) =
                futures::join!(resolver.discovered_targets(input.clone(), self.ctx), async {
                    while let Some(NotifierMessage { ty, msg }) = notification_stream.next().await {
                        let _ = notifier.update_status(ty, msg);
                    }
                });
            use ffx_diagnostics_analytics::ResultExt;
            let mut targets: Vec<discovery::TargetHandle> = targets
                .or_analytics(ffx_target::analytics::PointOfFailure::DiscoveryFailure {
                    query: Some(input.to_analytics_tag()),
                    discovery_sources: sources,
                })
                .await?;

            // If there are no devices and we specify something that can be interpreted as an
            // address, then add it to the discovered list.
            //
            // This should include addresses, VSock, USB, and will likely include things that will
            // be added in the future.
            if targets.is_empty()
                && let Some(addr) = input.get_target_addr()
            {
                let handle = TargetHandle {
                    node_name: None,
                    state: discovery::TargetState::Product { addrs: vec![addr], serial: None },
                    manual: true,
                };
                targets.push(handle);
            }

            if targets.is_empty() {
                let colors = Colors::current();
                notifier.info(format!(
                        "{}{}No matching devices were found.{} Ensure the diagnostics logs don't contain your device before proceeding to debugging.",
                        colors.bold,
                        colors.red,
                        colors.reset
                    ))?;
                notifier.info(
                        format!("The following link contains steps for general network debugging: https://fuchsia.dev/fuchsia-src/development/tools/ffx/workflows/network-connectivity")
                    )?;
                for (name, source) in sources.iter_names() {
                    // TODO(b/427299969): Style hinting should be supported here to remove the need
                    // for this caller to implement styling. For certain displays styling should
                    // be skipped entirely: in an infra environment there will be a lot of
                    // unreadable gibberish surrounding this message.
                    notifier.info(format!(
                        "{}{} failed to find matching devices.{}",
                        colors.red, name, colors.reset
                    ))?;
                    use ffx_diagnostics_formatting::AsDiagnosticMessage;
                    let additional_message = source.bits().as_diagnostic_message();
                    if !additional_message.is_empty() {
                        notifier.info(additional_message)?;
                    }
                }
                ffx_diagnostics_analytics::mark_point_of_failure(
                    ffx_target::analytics::PointOfFailure::NoMatchingTargets {
                        query: Some(input.to_analytics_tag()),
                        discovery_sources: sources,
                    },
                )
                .await;
                return Err(anyhow::anyhow!("Unable to find any matching devices"));
            }
            if targets.len() > 1 {
                ffx_diagnostics_analytics::mark_point_of_failure(
                    ffx_target::analytics::PointOfFailure::TooManyMatchingTargets {
                        query: Some(input.to_analytics_tag()),
                        discovery_sources: sources,
                    },
                )
                .await;
                return Err(anyhow::anyhow!(
                    "Too many targets. You may need to be more specific in the device you are checking. Found: {targets:?}"
                ));
            }
            Ok(targets.pop().unwrap())
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use discovery::TargetState;
    use futures::channel::mpsc::UnboundedSender;
    use std::sync::{Arc, Mutex};

    static MOCK_HANDLES_LOCK: std::sync::LazyLock<futures::lock::Mutex<()>> =
        std::sync::LazyLock::new(|| futures::lock::Mutex::new(()));
    static MOCK_HANDLES: std::sync::LazyLock<Arc<Mutex<Vec<TargetHandle>>>> =
        std::sync::LazyLock::new(|| Arc::new(Mutex::new(Vec::new())));

    #[derive(Default)]
    struct MockResolver;

    impl DiagnosticsResolver for MockResolver {
        fn from_sources_and_notifier_sender(
            _source: DiscoverySources,
            _notifier_sender: UnboundedSender<NotifierMessage>,
        ) -> Self {
            Self
        }

        async fn discovered_targets(
            self,
            _query: TargetInfoQuery,
            _ctx: &EnvironmentContext,
        ) -> fho::Result<Vec<TargetHandle>> {
            Ok(MOCK_HANDLES.lock().unwrap().clone())
        }
    }

    #[fuchsia::test]
    async fn test_resolve_target_success() {
        let _guard = MOCK_HANDLES_LOCK.lock().await;
        let env = ffx_config::test_env().build().unwrap();
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        {
            *MOCK_HANDLES.lock().unwrap() = vec![handle.clone()];
        }
        let mut check = ResolveTarget::<_, MockResolver>::new(&env.context);
        let res = check.check(TargetInfoQuery::First, &mut notifier).await.unwrap();
        assert_eq!(res, handle);
    }

    #[fuchsia::test]
    async fn test_resolve_target_no_devices_found() {
        let _guard = MOCK_HANDLES_LOCK.lock().await;
        {
            *MOCK_HANDLES.lock().unwrap() = vec![];
        }
        let env = ffx_config::test_env().build().unwrap();
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let mut check = ResolveTarget::<_, MockResolver>::new(&env.context);
        let res = check.check(TargetInfoQuery::First, &mut notifier).await;
        assert!(res.is_err());
    }

    #[fuchsia::test]
    async fn test_resolve_target_too_many_devices_found() {
        let _guard = MOCK_HANDLES_LOCK.lock().await;
        let env = ffx_config::test_env().build().unwrap();
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let handle1 = TargetHandle {
            node_name: Some("test-node-1".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let handle2 = TargetHandle {
            node_name: Some("test-node-2".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        {
            *MOCK_HANDLES.lock().unwrap() = vec![handle1, handle2];
        }
        let mut check = ResolveTarget::<_, MockResolver>::new(&env.context);
        let res = check.check(TargetInfoQuery::First, &mut notifier).await;
        assert!(res.is_err());
    }

    #[fuchsia::test]
    async fn test_notify_for_discovery_sources_emulator() {
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::EMU_INSTANCE_ROOT_DIR, "/tmp/emu-test-path")
            .build()
            .unwrap();
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let sources = DiscoverySources::EMULATOR;
        notify_for_discovery_sources(&env.context, sources, &mut notifier).unwrap();
        let output: String = notifier.into();
        assert!(output.contains("Searching for emulators at /tmp/emu-test-path"));
        assert!(!output.contains("fastboot"));
    }

    #[fuchsia::test]
    async fn test_notify_for_discovery_sources_fastboot_file_set() {
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::FASTBOOT_FILE_PATH, "/tmp/fastboot-test.json")
            .build()
            .unwrap();
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let sources = DiscoverySources::FASTBOOT_FILE;
        notify_for_discovery_sources(&env.context, sources, &mut notifier).unwrap();
        let output: String = notifier.into();
        assert!(output.contains("Checking fastboot file at /tmp/fastboot-test.json"));
        assert!(!output.contains("emulators"));
    }

    #[fuchsia::test]
    async fn test_notify_for_discovery_sources_both() {
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::EMU_INSTANCE_ROOT_DIR, "/tmp/emu-test-path")
            .runtime_config(ffx_config::keys::FASTBOOT_FILE_PATH, "/tmp/fastboot-test.json")
            .build()
            .unwrap();
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let sources = DiscoverySources::EMULATOR | DiscoverySources::FASTBOOT_FILE;
        notify_for_discovery_sources(&env.context, sources, &mut notifier).unwrap();
        let output: String = notifier.into();
        assert!(output.contains("Searching for emulators at /tmp/emu-test-path"));
        assert!(output.contains("Checking fastboot file at /tmp/fastboot-test.json"));
    }

    #[fuchsia::test]
    async fn test_notify_for_discovery_sources_none() {
        let env = ffx_config::test_env().build().unwrap();
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let sources = DiscoverySources::MANUAL;
        notify_for_discovery_sources(&env.context, sources, &mut notifier).unwrap();
        let output: String = notifier.into();
        assert!(output.is_empty());
    }

    #[fuchsia::test]
    async fn test_resolve_target_escapes_control_characters() {
        let _guard = MOCK_HANDLES_LOCK.lock().await;
        let env = ffx_config::test_env().build().unwrap();
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let handle = TargetHandle {
            node_name: Some("malicious\x1b[31m_node\n\r\0".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        {
            *MOCK_HANDLES.lock().unwrap() = vec![handle.clone()];
        }
        let check = ResolveTarget::<_, MockResolver>::new(&env.context);
        let query = TargetInfoQuery::NodenameOrId("target\x1b[2J_query".to_string());
        let res = check.check_with_notifier(query, &mut notifier).await.unwrap();
        assert_eq!(res.0, handle);

        let output: String = notifier.into();
        assert!(!output.contains('\x1b'));
        assert!(!output.contains('\0'));
        assert!(!output.contains('\r'));
        assert!(output.contains("target\\u{1b}[2J_query"));
        assert!(output.contains("malicious\\u{1b}[31m_node\\n\\r\\u{0}"));
    }
}
