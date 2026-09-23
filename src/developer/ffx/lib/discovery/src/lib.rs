// Copyright 2021 The Fuchsia Authors. All rights 1eserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use crate::desc::Description;
use crate::emulator_watcher::EmulatorWatcher;
use crate::error::Result;
pub use crate::events::{
    FastbootConnectionState, FastbootTargetState, TargetEvent, TargetHandle, TargetState,
};
use crate::fastboot_file_watcher::FastbootWatcher;
use crate::gce_watcher::GceWatcher;
use crate::query::TargetInfoQuery;
use crate::usb_vsock_watcher::UsbVsockWatcher;
use bitflags::bitflags;
use ffx_config::EnvironmentContext;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use futures::{FutureExt, Stream, StreamExt};
use manual_targets::watcher::{
    ManualTargetEvent, ManualTargetEventHandler, ManualTargetWatcher,
    recommended_watcher as manual_recommended_watcher,
};
use mdns_discovery::{MdnsEventHandler, MdnsWatcher, recommended_watcher};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use usb_fastboot_discovery::{
    FastbootEvent, FastbootEventHandler, FastbootUsbWatcher,
    recommended_watcher as fastboot_watcher,
};
// TODO(colnnelson): Long term it would be nice to have this be pulled into the mDNS library
// so that it can speak our language. Or even have the mdns library not export FIDL structs
// but rather some other well-defined type
use fidl_fuchsia_developer_ffx as ffx;

pub mod desc;
pub mod emulator_watcher;
pub mod error;
pub mod events;
pub mod fastboot_file_watcher;
pub mod gce_watcher;
pub mod instance_watcher;
mod merge;
pub mod query;
mod usb_vsock_watcher;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

#[allow(dead_code)]
/// A stream of new devices as they appear on the bus. See [`wait_for_devices`].
pub struct TargetStream {
    /// Watches mdns events
    mdns_watcher: Option<MdnsWatcher>,

    /// Watches for FastbootUsb events
    fastboot_usb_watcher: Option<FastbootUsbWatcher>,

    /// Watches for USB VSOCK events
    usb_vsock_watcher: Option<UsbVsockWatcher>,

    /// Watches for ManualTarget events
    manual_targets_watcher: Option<ManualTargetWatcher>,

    /// Watches for Emulator events
    emulator_watcher: Option<EmulatorWatcher>,

    /// Watches for Emulator events
    fastboot_file_watcher: Option<FastbootWatcher>,

    /// Watches for GCE instance events
    gce_watcher: Option<GceWatcher>,

    /// This is where results from the various watchers are published.
    queue: UnboundedReceiver<TargetEvent>,
}

pub struct TargetStreamConfig<Mdns, Fusb, Man>
where
    Mdns: MdnsEventHandler,
    Fusb: FastbootEventHandler,
    Man: ManualTargetEventHandler,
{
    /// MDNS event handler.
    pub mdns_event_handler: Option<Mdns>,

    /// Fastboot USB event handler.
    pub fastboot_event_handler: Option<Fusb>,

    /// Manual target watcher.
    pub manual_targets_event_handler: Option<Man>,

    /// Emulator watcher.
    pub emulator_watcher: Option<EmulatorWatcher>,

    /// Watches for USB VSOCK events
    pub usb_vsock_watcher: Option<UsbVsockWatcher>,

    /// Fastboot file watcher.
    pub fastboot_file_watcher: Option<FastbootWatcher>,

    /// GCE watcher.
    pub gce_watcher: Option<GceWatcher>,
}

impl<Mdns, Fusb, Man> TargetStreamConfig<Mdns, Fusb, Man>
where
    Mdns: MdnsEventHandler,
    Fusb: FastbootEventHandler,
    Man: ManualTargetEventHandler,
{
    pub fn new() -> Self {
        // The type constraints make doing a derive of Default not doable.
        Self {
            mdns_event_handler: None,
            fastboot_event_handler: None,
            manual_targets_event_handler: None,
            emulator_watcher: None,
            usb_vsock_watcher: None,
            fastboot_file_watcher: None,
            gce_watcher: None,
        }
    }

    pub fn set_mdns_event_handler(&mut self, e: Mdns) {
        self.mdns_event_handler = Some(e);
    }

    pub fn set_fastboot_event_handler(&mut self, e: Fusb) {
        self.fastboot_event_handler = Some(e);
    }

    pub fn set_manual_event_handler(&mut self, e: Man) {
        self.manual_targets_event_handler = Some(e);
    }

    pub fn set_emulator_watcher(&mut self, e: EmulatorWatcher) {
        self.emulator_watcher = Some(e);
    }

    pub fn set_usb_vsock_watcher(&mut self, e: UsbVsockWatcher) {
        self.usb_vsock_watcher = Some(e);
    }

    pub fn set_fastboot_file_watcher(&mut self, f: FastbootWatcher) {
        self.fastboot_file_watcher = Some(f)
    }

    pub fn set_gce_watcher(&mut self, g: GceWatcher) {
        self.gce_watcher = Some(g);
    }
}

impl TargetStream {
    /// Constructs a new target stream using the config, with each watcher defaulting to the
    /// recommended one.
    pub fn new<M, F, Man>(
        context: &EnvironmentContext,
        config: TargetStreamConfig<M, F, Man>,
        queue: UnboundedReceiver<TargetEvent>,
    ) -> Self
    where
        M: MdnsEventHandler,
        F: FastbootEventHandler,
        Man: ManualTargetEventHandler,
    {
        Self {
            mdns_watcher: config.mdns_event_handler.map(|e| recommended_watcher(e)),
            fastboot_usb_watcher: config.fastboot_event_handler.map(|e| fastboot_watcher(e)),
            manual_targets_watcher: config
                .manual_targets_event_handler
                .map(|e| manual_recommended_watcher(context, e)),
            emulator_watcher: config.emulator_watcher,
            usb_vsock_watcher: config.usb_vsock_watcher,
            fastboot_file_watcher: config.fastboot_file_watcher,
            gce_watcher: config.gce_watcher,
            queue,
        }
    }
}

pub struct DiscoveryBuilder {
    emulator_instance_root: Option<PathBuf>,
    fastboot_devices_file_path: Option<PathBuf>,
    usb_vsock_driver_socket_path: Option<PathBuf>,
    gce_instance_root: Option<PathBuf>,
    sources: DiscoverySources,
    timeout: Option<Duration>,
    state_filter: TargetStateFilter,
    short_circuit_on_first: bool,
}

impl DiscoveryBuilder {
    pub fn set_source(mut self, source: DiscoverySources) -> Self {
        self.sources = source;
        self
    }

    #[cfg(test)]
    pub fn with_source(mut self, source: DiscoverySources) -> Self {
        self.sources.insert(source);
        self
    }

    pub fn with_emulator_instance_root(mut self, emulator_instance_root: Option<PathBuf>) -> Self {
        if emulator_instance_root.is_some() {
            self.emulator_instance_root = emulator_instance_root;
            self.sources.insert(DiscoverySources::EMULATOR);
        }
        self
    }

    pub fn with_fastboot_devices_file_path(
        mut self,
        fastboot_devices_file_path: Option<PathBuf>,
    ) -> Self {
        if fastboot_devices_file_path.is_some() {
            self.fastboot_devices_file_path = fastboot_devices_file_path;
            self.sources.insert(DiscoverySources::FASTBOOT_FILE);
        }
        self
    }

    pub fn with_usb_vsock_driver_socket_path(
        mut self,
        usb_vsock_driver_socket_path: Option<PathBuf>,
    ) -> Self {
        if usb_vsock_driver_socket_path.is_some() {
            self.usb_vsock_driver_socket_path = usb_vsock_driver_socket_path;
            self.sources.insert(DiscoverySources::USB_VSOCK);
        }
        self
    }

    pub fn with_gce_instance_root(mut self, gce_instance_root: Option<PathBuf>) -> Self {
        if gce_instance_root.is_some() {
            self.gce_instance_root = gce_instance_root;
            self.sources.insert(DiscoverySources::GCE);
        }
        self
    }

    /// Specify the timeout in milliseconds. (Specified as u64 instead of
    /// Duration because the value will normally come from config, so we'll do
    /// the conversion here rather then having every caller do it.)
    pub fn with_timeout_msecs(mut self, timeout_msecs: Option<u64>) -> Self {
        self.timeout = timeout_msecs.map(Duration::from_millis);
        self
    }

    /// Filter discovered targets by target state.
    pub fn with_state_filter(mut self, state_filter: TargetStateFilter) -> Self {
        self.state_filter = state_filter;
        self
    }

    /// Control whether `TargetInfoQuery::First` short-circuits on the first match.
    /// By default (`false`), `First` waits for the discovery window to complete to detect ambiguity.
    pub fn with_short_circuit_on_first(mut self, short_circuit_on_first: bool) -> Self {
        self.short_circuit_on_first = short_circuit_on_first;
        self
    }

    pub fn build(self, context: &EnvironmentContext) -> Discovery {
        Discovery {
            emulator_instance_root: self.emulator_instance_root,
            fastboot_devices_file_path: self.fastboot_devices_file_path,
            usb_vsock_driver_socket_path: self.usb_vsock_driver_socket_path,
            gce_instance_root: self.gce_instance_root,
            sources: self.sources,
            timeout: self.timeout,
            state_filter: self.state_filter,
            short_circuit_on_first: self.short_circuit_on_first,
            stream: Mutex::new(None),
            context: context.clone(),
        }
    }

    /// Builds the discovery stream with a dependency-injected stream.
    ///
    /// Keep in mind that this means most of the fields in the builder _except for the timeout_ will
    /// be ignored:
    /// -- emulator_instance_root
    /// -- fastboot_devices_file_path
    /// -- sources
    pub fn build_with_stream<S>(self, context: &EnvironmentContext, stream: S) -> Discovery
    where
        S: Stream<Item = TargetEvent> + Unpin + Send + Sync + 'static,
    {
        let res = self.build(context);
        let inner_stream: Box<dyn Stream<Item = TargetEvent> + Unpin + Send + Sync> =
            if let Some(t) = res.timeout {
                Box::new(stream.take_until(fuchsia_async::Timer::new(t)))
            } else {
                Box::new(stream)
            };
        res.stream.lock().unwrap().replace(inner_stream);
        res
    }
}

impl Default for DiscoveryBuilder {
    fn default() -> Self {
        Self {
            emulator_instance_root: None,
            fastboot_devices_file_path: None,
            usb_vsock_driver_socket_path: None,
            gce_instance_root: None,
            sources: DiscoverySources::default(),
            timeout: Some(DEFAULT_TIMEOUT),
            state_filter: TargetStateFilter::default(),
            short_circuit_on_first: false,
        }
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct TargetStateFilter: u8 {
        const PRODUCT = 1 << 0;
        const FASTBOOT = 1 << 1;
        const ZEDBOOT = 1 << 2;
        const UNKNOWN = 1 << 3;
    }
}

impl Default for TargetStateFilter {
    fn default() -> Self {
        TargetStateFilter::all()
    }
}

impl TargetStateFilter {
    pub fn matches(&self, state: &TargetState) -> bool {
        match state {
            TargetState::Product { .. } => self.contains(TargetStateFilter::PRODUCT),
            TargetState::Fastboot(_) => self.contains(TargetStateFilter::FASTBOOT),
            TargetState::Zedboot => self.contains(TargetStateFilter::ZEDBOOT),
            TargetState::Unknown => self.contains(TargetStateFilter::UNKNOWN),
        }
    }
}

pub struct Discovery {
    emulator_instance_root: Option<PathBuf>,
    fastboot_devices_file_path: Option<PathBuf>,
    usb_vsock_driver_socket_path: Option<PathBuf>,
    gce_instance_root: Option<PathBuf>,
    sources: DiscoverySources,
    timeout: Option<Duration>,
    state_filter: TargetStateFilter,
    short_circuit_on_first: bool,
    // For testing purposes, we can provide an arbitrary stream.
    // For example, in the testing module `setup_test()` uses this to store
    // a `Vec<_>` stream iterator.
    stream: Mutex<Option<Box<dyn Stream<Item = TargetEvent> + Unpin + Send + Sync>>>,
    context: EnvironmentContext,
}

impl Discovery {
    pub fn sources(&self) -> DiscoverySources {
        self.sources
    }

    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    // Discover devices via mDNS broadcast, etc, with a time limit
    fn create_stream(&self) -> Result<Pin<Box<dyn Stream<Item = TargetEvent> + Send>>> {
        if let Some(stream) = self.stream.lock().unwrap().take() {
            // In tests, we'll just use the provided stream
            return Ok(Box::pin(stream));
        }
        let stream = wait_for_devices(
            &self.context,
            self.emulator_instance_root.clone(),
            self.fastboot_devices_file_path.clone(),
            self.usb_vsock_driver_socket_path.clone(),
            self.gce_instance_root.clone(),
            self.sources,
        )?;
        if let Some(timeout) = self.timeout {
            let timer = fuchsia_async::Timer::new(timeout);
            Ok(Box::pin(stream.take_until(timer)))
        } else {
            Ok(Box::pin(stream))
        }
    }

    // Create a stream that is limited by a timer, and will short-circuit query matches:
    // If the match is not "First" (or short_circuit_on_first is true), then close the stream on the first match.
    // Otherwise, close the stream when the timer runs out.
    pub fn discovery_stream(
        &self,
        query: TargetInfoQuery,
    ) -> Result<impl Stream<Item = TargetEvent> + Send + use<>> {
        let stream = self.create_stream()?;
        // The logic here is tricky. We want to close the stream as _soon_
        // as we see a matching query, rather than, say, using scan()
        // to close it on the _next_ event. (Because we may only see the
        // one discovery event, before the timer.) So we'll use a oneshot
        // to create a future that we'll use with stream.take_until().
        //
        // Getting the oneshot tx into the closure is also tricky,
        // due to move semantics, etc. We have to make sure it doesn't
        // get Dropped early, so we wrap it in an Arc<Mutex<>>.
        let (single_target_tx, single_target_rx) = futures::channel::oneshot::channel();
        let single_target_tx = Arc::new(Mutex::new(Some(single_target_tx)));
        let is_indefinite = self.timeout.is_none();
        let state_filter = self.state_filter;
        let short_circuit_on_first = self.short_circuit_on_first || is_indefinite;
        Ok(stream
            .filter_map(move |ev| {
                let query = query.clone();
                let sender = Arc::clone(&single_target_tx);
                async move {
                    let th = ev.target_handle();
                    // Only match against the query and state filter
                    if state_filter.matches(&th.state) && query.match_handle(th) {
                        // When we add a handle that matches our query, fire the oneshot if not First
                        // or if short_circuit_on_first is true.
                        if matches!(ev, TargetEvent::Added(_))
                            && (!matches!(query, TargetInfoQuery::First) || short_circuit_on_first)
                        {
                            // We'll only need the oneshot once
                            if let Some(s) = sender.lock().unwrap().take() {
                                let _ = s.send(());
                            }
                        }
                        Some(ev)
                    } else {
                        None
                    }
                }
                .boxed()
            })
            .take_until(single_target_rx))
    }

    async fn build_devices_from_stream(
        &self,
        mut stream: impl Stream<Item = TargetEvent> + std::marker::Unpin,
    ) -> Vec<TargetHandle> {
        let mut target_set = merge::TargetSet::new();
        while let Some(ev) = stream.next().await {
            target_set.process_event(ev);
        }
        target_set.into_targets()
    }

    pub async fn discover_devices(&self, query: TargetInfoQuery) -> Result<Vec<TargetHandle>> {
        let stream = self.discovery_stream(query)?;
        Ok(self.build_devices_from_stream(stream).await)
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct DiscoverySources: u8 {
        const MDNS = 1 << 0;
        const USB_FASTBOOT = 1 << 1;
        const MANUAL = 1 << 2;
        const EMULATOR = 1 << 3;
        const FASTBOOT_FILE = 1 << 4;
        const USB_VSOCK = 1 << 5;
        const GCE = 1 << 6;
    }
}

impl Default for DiscoverySources {
    fn default() -> Self {
        DiscoverySources::all()
    }
}

fn wait_for_devices(
    context: &EnvironmentContext,
    emulator_instance_root: Option<PathBuf>,
    fastboot_devices_file_path: Option<PathBuf>,
    usb_vsock_driver_socket_path: Option<PathBuf>,
    gce_instance_root: Option<PathBuf>,
    sources: DiscoverySources,
) -> Result<TargetStream> {
    let mut config = TargetStreamConfig::new();
    let (sender, queue) = unbounded();
    if sources.contains(DiscoverySources::MDNS) {
        let mdns_sender = sender.clone();
        config.set_mdns_event_handler(move |res: ffx::MdnsEventType| {
            // Translate the result to a TargetEvent
            let event = TargetEvent::try_from(res);
            if let Ok(event) = event {
                let _ = mdns_sender.unbounded_send(event);
            }
        })
    }

    // USB Fastboot watcher
    if sources.contains(DiscoverySources::USB_FASTBOOT) {
        let fastboot_sender = sender.clone();
        config.set_fastboot_event_handler(move |res: FastbootEvent| {
            // Translate the result to a TargetEvent
            log::debug!("discovery watcher got fastboot event: {:#?}", res);
            let event = res.into();
            let _ = fastboot_sender.unbounded_send(event);
        })
    }

    // USB VSOCK watcher
    if let Some(socket_path) = usb_vsock_driver_socket_path
        && sources.contains(DiscoverySources::USB_VSOCK)
    {
        let usb_vsock_sender = sender.clone();
        config.set_usb_vsock_watcher(UsbVsockWatcher::new(socket_path, usb_vsock_sender));
    }

    if sources.contains(DiscoverySources::MANUAL) {
        let manual_targets_sender = sender.clone();
        config.set_manual_event_handler(move |res: ManualTargetEvent| {
            // Translate the result to a TargetEvent
            log::trace!("discovery watcher got manual target event: {:#?}", res);
            let event = res.into();
            let _ = manual_targets_sender.unbounded_send(event);
        })
    }

    if sources.contains(DiscoverySources::EMULATOR) {
        if let Some(instance_root) = emulator_instance_root {
            config.set_emulator_watcher(EmulatorWatcher::new(instance_root, sender.clone())?)
        }
    }

    if sources.contains(DiscoverySources::FASTBOOT_FILE) {
        if let Some(fastboot_devices_file) = fastboot_devices_file_path {
            config.set_fastboot_file_watcher(FastbootWatcher::new(
                fastboot_devices_file,
                sender.clone(),
            )?)
        }
    }

    if sources.contains(DiscoverySources::GCE) {
        if let Some(instance_root) = gce_instance_root {
            config.set_gce_watcher(GceWatcher::new(instance_root, sender)?)
        }
    }

    Ok(TargetStream::new(context, config, queue))
}

impl Stream for TargetStream {
    type Item = TargetEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.queue).poll_next(cx)
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use crate::{TargetHandle, TargetInfoQuery, TargetState};
    use addr::TargetAddr;
    use pretty_assertions::assert_eq;
    use std::fs::File;
    use std::io::Write;
    use std::str::FromStr;

    static_assertions::assert_impl_all!(Discovery: Send, Sync);

    fn setup_test(context: &EnvironmentContext) -> (Discovery, TargetHandle, TargetHandle) {
        let handle1 = TargetHandle {
            node_name: Some("test-target-1".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let handle2 = TargetHandle {
            node_name: Some("test-target-2".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let events = vec![TargetEvent::Added(handle1.clone()), TargetEvent::Added(handle2.clone())];
        let stream = Box::new(futures::stream::iter(events));
        let discovery = DiscoveryBuilder::default().build_with_stream(context, stream);
        (discovery, handle1, handle2)
    }

    ///////////////////////////////////////////////////////////////////////////
    // Test DiscoveryBuilder
    ///////////////////////////////////////////////////////////////////////////

    #[test]
    fn test_discovery_builder_default() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let discovery = DiscoveryBuilder::default().build(&env.context);
        assert_eq!(discovery.sources, DiscoverySources::all());
        assert!(discovery.emulator_instance_root.is_none());
    }

    #[test]
    fn test_discovery_builder_changes() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let discovery = DiscoveryBuilder::default()
            .set_source(DiscoverySources::MANUAL)
            .with_source(DiscoverySources::EMULATOR)
            .set_source(DiscoverySources::MDNS)
            .with_source(DiscoverySources::USB_FASTBOOT)
            .build(&env.context);
        assert_eq!(discovery.sources, DiscoverySources::USB_FASTBOOT | DiscoverySources::MDNS);
        assert!(discovery.emulator_instance_root.is_none());
    }

    #[test]
    fn test_discovery_builder_with_root() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let discovery = DiscoveryBuilder::default()
            .set_source(DiscoverySources::MANUAL)
            .with_emulator_instance_root(Some(
                PathBuf::from_str("/tmp").expect("tmp is a valid path"),
            ))
            .build(&env.context);

        assert_eq!(discovery.sources, DiscoverySources::MANUAL | DiscoverySources::EMULATOR);
        assert_eq!(
            discovery.emulator_instance_root,
            Some(PathBuf::from_str("/tmp").expect("tmp is a valid path"))
        );
    }

    #[test]
    fn test_discovery_builder_with_gce_root() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let discovery = DiscoveryBuilder::default()
            .set_source(DiscoverySources::MANUAL)
            .with_gce_instance_root(Some(PathBuf::from_str("/tmp").expect("tmp is a valid path")))
            .build(&env.context);

        assert_eq!(discovery.sources, DiscoverySources::MANUAL | DiscoverySources::GCE);
        assert_eq!(
            discovery.gce_instance_root,
            Some(PathBuf::from_str("/tmp").expect("tmp is a valid path"))
        );
    }

    ///////////////////////////////////////////////////////////////////////////
    ///  TargetStream tests
    ///////////////////////////////////////////////////////////////////////////

    #[fuchsia::test]
    async fn test_target_stream() {
        let (sender, queue) = unbounded();

        let mut stream = TargetStream {
            mdns_watcher: None,
            fastboot_usb_watcher: None,
            manual_targets_watcher: None,
            usb_vsock_watcher: None,
            emulator_watcher: None,
            fastboot_file_watcher: None,
            gce_watcher: None,
            queue,
        };

        // Send a few events
        sender
            .unbounded_send(TargetEvent::Added(TargetHandle {
                node_name: Some("Vin".to_string()),
                state: TargetState::Zedboot,
                manual: false,
            }))
            .unwrap();

        sender
            .unbounded_send(TargetEvent::Removed(TargetHandle {
                node_name: Some("Vin".to_string()),
                state: TargetState::Zedboot,
                manual: false,
            }))
            .unwrap();

        assert_eq!(
            stream.next().await.unwrap(),
            TargetEvent::Added(TargetHandle {
                node_name: Some("Vin".to_string()),
                state: TargetState::Zedboot,
                manual: false,
            })
        );

        assert_eq!(
            stream.next().await.unwrap(),
            TargetEvent::Removed(TargetHandle {
                node_name: Some("Vin".to_string()),
                state: TargetState::Zedboot,
                manual: false,
            })
        );
    }

    #[fuchsia::test]
    async fn test_discover_devices() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let handle = TargetHandle {
            node_name: Some("test-target".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let events = vec![TargetEvent::Added(handle.clone())];
        let stream = Box::new(futures::stream::iter(events));
        let discovery = DiscoveryBuilder::default().build_with_stream(&env.context, stream);
        let targets = discovery.discover_devices(TargetInfoQuery::First).await.unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0], handle);
    }

    #[fuchsia::test]
    async fn test_devices_filtered_short_circuits() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let (discovery, handle1, _) = setup_test(&env.context);
        let mut stream = discovery
            .discovery_stream(TargetInfoQuery::NodenameOrId("test-target-1".to_string()))
            .unwrap();

        // We should get the first handle, and then the stream should be closed.
        assert_eq!(stream.next().await.unwrap().target_handle(), &handle1);
        assert!(stream.next().await.is_none());
    }

    #[fuchsia::test]
    async fn test_devices_filtered_first_query() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let (discovery, handle1, handle2) = setup_test(&env.context);
        let mut stream = discovery.discovery_stream(TargetInfoQuery::First).unwrap();

        // We should get both handles.
        assert_eq!(stream.next().await.unwrap().target_handle(), &handle1);
        assert_eq!(stream.next().await.unwrap().target_handle(), &handle2);
        assert!(stream.next().await.is_none());
    }

    // Tests that when discovery is configured with an indefinite timeout (`timeout: None`),
    // querying for the default/first target (`TargetInfoQuery::First`) short-circuits as
    // soon as the first target is added. This ensures that callers waiting indefinitely for a
    // target to appear will return immediately upon detecting the device, rather than hanging
    // indefinitely on an open stream.
    #[fuchsia::test]
    async fn test_indefinite_discovery_stream_first_query() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let handle1 = TargetHandle {
            node_name: Some("test-target-1".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let handle2 = TargetHandle {
            node_name: Some("test-target-2".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        sender.unbounded_send(TargetEvent::Added(handle1.clone())).unwrap();
        sender.unbounded_send(TargetEvent::Added(handle2.clone())).unwrap();
        let discovery = DiscoveryBuilder::default()
            .with_timeout_msecs(None)
            .build_with_stream(&env.context, receiver);
        let mut stream = discovery.discovery_stream(TargetInfoQuery::First).unwrap();

        // Under indefinite timeout, First should short-circuit on the first added device.
        assert_eq!(stream.next().await.unwrap().target_handle(), &handle1);
        assert!(stream.next().await.is_none());
    }

    #[fuchsia::test]
    async fn test_discovery_stream_with_state_filter_ignores_product_and_finds_fastboot() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let product_handle = TargetHandle {
            node_name: Some("test-target".to_string()),
            state: TargetState::Product { addrs: vec![], serial: Some("fb-123".to_string()) },
            manual: false,
        };
        let fastboot_handle = TargetHandle {
            node_name: Some("".to_string()),
            state: TargetState::Fastboot(FastbootTargetState {
                serial_number: "fb-123".to_string(),
                connection_state: FastbootConnectionState::Usb,
            }),
            manual: false,
        };
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        sender.unbounded_send(TargetEvent::Added(product_handle.clone())).unwrap();
        sender.unbounded_send(TargetEvent::Added(fastboot_handle.clone())).unwrap();
        let discovery = DiscoveryBuilder::default()
            .with_state_filter(TargetStateFilter::FASTBOOT)
            .with_timeout_msecs(None)
            .build_with_stream(&env.context, receiver);
        let mut stream =
            discovery.discovery_stream(TargetInfoQuery::Id("fb-123".to_string())).unwrap();

        // The Product handle must be ignored; the Fastboot handle must be returned.
        assert_eq!(stream.next().await.unwrap().target_handle(), &fastboot_handle);
        assert!(stream.next().await.is_none());
    }

    #[fuchsia::test]
    async fn test_discovery_stream_with_state_filter_matches_nodename_ignores_product() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let product_handle = TargetHandle {
            node_name: Some("test-target".to_string()),
            state: TargetState::Product { addrs: vec![], serial: None },
            manual: false,
        };
        let fastboot_handle = TargetHandle {
            node_name: Some("test-target".to_string()),
            state: TargetState::Fastboot(FastbootTargetState {
                serial_number: "fb-456".to_string(),
                connection_state: FastbootConnectionState::Usb,
            }),
            manual: false,
        };
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        sender.unbounded_send(TargetEvent::Added(product_handle.clone())).unwrap();
        sender.unbounded_send(TargetEvent::Added(fastboot_handle.clone())).unwrap();
        let discovery = DiscoveryBuilder::default()
            .with_state_filter(TargetStateFilter::FASTBOOT)
            .with_timeout_msecs(None)
            .build_with_stream(&env.context, receiver);
        let mut stream = discovery
            .discovery_stream(TargetInfoQuery::NodenameOrId("test-target".to_string()))
            .unwrap();

        // The Product handle matching nodename must NOT trigger short-circuit;
        // only the Fastboot handle must be returned.
        assert_eq!(stream.next().await.unwrap().target_handle(), &fastboot_handle);
        assert!(stream.next().await.is_none());
    }

    #[fuchsia::test]
    async fn test_discovery_stream_first_short_circuits_when_configured() {
        let env = ffx_config::test_env().build().expect("Test Env Init");
        let fastboot_handle1 = TargetHandle {
            node_name: Some("".to_string()),
            state: TargetState::Fastboot(FastbootTargetState {
                serial_number: "fb-1".to_string(),
                connection_state: FastbootConnectionState::Usb,
            }),
            manual: false,
        };
        let fastboot_handle2 = TargetHandle {
            node_name: Some("".to_string()),
            state: TargetState::Fastboot(FastbootTargetState {
                serial_number: "fb-2".to_string(),
                connection_state: FastbootConnectionState::Usb,
            }),
            manual: false,
        };
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        sender.unbounded_send(TargetEvent::Added(fastboot_handle1.clone())).unwrap();
        sender.unbounded_send(TargetEvent::Added(fastboot_handle2.clone())).unwrap();
        // Even with a long timeout (100 seconds), First must short-circuit when configured.
        let discovery = DiscoveryBuilder::default()
            .with_state_filter(TargetStateFilter::FASTBOOT)
            .with_short_circuit_on_first(true)
            .with_timeout_msecs(Some(100000))
            .build_with_stream(&env.context, receiver);
        let mut stream = discovery.discovery_stream(TargetInfoQuery::First).unwrap();

        assert_eq!(stream.next().await.unwrap().target_handle(), &fastboot_handle1);
        assert!(stream.next().await.is_none());
    }

    fn build_instance_file(dir: &PathBuf, name: &str) -> std::io::Result<File> {
        let new_instance_dir = dir.join(String::from(name));
        std::fs::create_dir_all(&new_instance_dir)?;
        let new_instance_engine_file = new_instance_dir.join("engine.json");
        use emulator_instance::EmulatorInstanceInfo;
        // Build the expected config JSON contents
        let mut instance_data = emulator_instance::EmulatorInstanceData::new_with_state(
            name,
            emulator_instance::EngineState::Running,
        );
        instance_data.set_pid(std::process::id());
        let config = instance_data.get_emulator_configuration_mut();
        config.host.networking = emulator_instance::NetworkingMode::User;
        config.host.port_map.insert(
            String::from("ssh"),
            emulator_instance::PortMapping { guest: 22, host: Some(3322) },
        );
        let config_str = serde_json::to_string(&instance_data)?;
        let mut config_file = File::create(&new_instance_engine_file)?;
        config_file.write_all(config_str.as_bytes())?;
        config_file.flush()?;
        Ok(config_file)
    }

    // This test has a race condition, which I (slgrady) have spent hours trying to find.
    // Apparently the notify crate in emulator_instance is for some reason not producing
    // the Create event for the new emulator file. The event is created in a separate
    // thread, so it's not an async issue. The watcher is not being dropped at that point.
    // The file is in fact created and placed on the filesystem; the watcher thread
    // is running at the point we are waiting for the event.  Giving up, since I believe
    // the race-condition only comes up in the artificial environment of a test case.
    // Normally, emulators are extended events, not just a fast creation of a single file.
    #[ignore]
    #[fuchsia::test]
    async fn test_target_stream_produces_emulator() {
        use tempfile::tempdir;
        let env = ffx_config::test_init().expect("Failed to initialize test env");

        // Create the emulator instance dir
        let temp = tempdir().expect("cannot get tempdir");
        let instance_dir = temp.path().to_path_buf();
        let emu_instances = emulator_instance::EmulatorInstances::new(instance_dir.clone());

        // Add a new emulator
        let config_file = build_instance_file(&instance_dir, "emu-data-instance").unwrap();

        // Before waiting on devices, let's make sure we're actually getting the
        // emulator. (This shouldn't be necessary, but I've seen this test flake
        // by timing out, so this is a validity check.)
        let existing = emulator_instance::get_all_targets(&emu_instances).unwrap();
        assert_eq!(existing.len(), 1);

        // Start watching the directory
        let mut stream = wait_for_devices(
            &env.context,
            Some(instance_dir.clone()),
            None,
            None,
            None,
            DiscoverySources::EMULATOR,
        )
        .unwrap();

        // Assert that the existing emulator is discovered
        let next =
            stream.next().await.expect("No event was waiting after watching for existing emulator");
        assert_eq!(
            next,
            // The node_name and the state both have to match the contents of the emu_config above.
            TargetEvent::Added(TargetHandle {
                // Name must correspond to "runtime:name" value in config
                node_name: Some("emu-data-instance".to_string()),
                // Addr must correspond to "host:port_map:sh:host" value in config
                state: TargetState::Product {
                    addrs: vec![TargetAddr::from_str("127.0.0.1:3322").unwrap()],
                    serial: None
                },
                manual: false,
            })
        );

        // Add a new (different) emulator
        let config_file2 = build_instance_file(&instance_dir, "emu-data-instance2").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        // let existing = emulator_instance::get_all_targets().await?;
        // assert_eq!(existing.len(), 2);

        // Assert that the newly-created emulator is discovered
        let next =
            stream.next().await.expect("No event was waiting after watching for new emulator");
        assert_eq!(
            next,
            // The node_name and the state both have to match the contents of the emu_config above.
            TargetEvent::Added(TargetHandle {
                // Name must correspond to "runtime:name" value in config
                node_name: Some("emu-data-instance2".to_string()),
                // Addr must correspond to "host:port_map:sh:host" value in config
                state: TargetState::Product {
                    addrs: vec![TargetAddr::from_str("127.0.0.1:3322").unwrap()],
                    serial: None
                },
                manual: false,
            })
        );

        drop(config_file);
        drop(config_file2);
        std::fs::remove_dir_all(&instance_dir).unwrap();
        // TODO(325325761) -- re-enable when emulator Remove events are generated
        // correctly.
        // let next = stream
        //     .next()
        //     .await
        //     .unwrap();
        // let next = next.expect("Getting emulator event failed");
        // assert_eq!(
        //     next,
        //     // The node_name and the state both have to match the contents of the emu_config above.
        //     TargetEvent::Removed(TargetHandle {
        //         // Name must correspond to "runtime:name" value in config
        //         node_name: Some("fuchsia-emulator".to_string()),
        //         // Addr must correspond to "host:port_map:sh:host" value in config
        //         state: TargetState::Product(TargetAddr::from_str("127.0.0.1:33881")?),
        //     })
        // );
    }
}
