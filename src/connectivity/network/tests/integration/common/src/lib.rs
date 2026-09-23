// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]

//! Provides utilities for Netstack integration tests.

pub mod constants;
pub mod devices;
pub mod dhcpv4;
pub mod interfaces;
pub mod ndp;
pub mod nud;
pub mod packets;
pub mod pcap;
pub mod ping;
#[macro_use]
pub mod realms;

use anyhow::Context as _;
use component_events::events::EventStream;
use diagnostics_hierarchy::{DiagnosticsHierarchy, HierarchyMatcher, filter_hierarchy};
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_diagnostics::Selector;
use fidl_fuchsia_inspect_deprecated::InspectMarker;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_netemul as fnetemul;
use fuchsia_async::{self as fasync, DurationExt as _};
use fuchsia_component::client;
use futures::future::FutureExt as _;
use futures::stream::{Stream, StreamExt as _, TryStreamExt as _};
use futures::{Future, select};
use std::pin::pin;

use crate::realms::TestSandboxExt as _;

/// An alias for `Result<T, anyhow::Error>`.
pub type Result<T = ()> = std::result::Result<T, anyhow::Error>;

/// Extra time to use when waiting for an async event to occur.
///
/// A large timeout to help prevent flakes.
pub const ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT: zx::MonotonicDuration =
    zx::MonotonicDuration::from_seconds(120);

/// Extra time to use when waiting for an async event to not occur.
///
/// Since a negative check is used to make sure an event did not happen, its okay to use a
/// smaller timeout compared to the positive case since execution stall in regards to the
/// monotonic clock will not affect the expected outcome.
pub const ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT: zx::MonotonicDuration =
    zx::MonotonicDuration::from_seconds(5);

/// The time to wait between two consecutive checks of an event.
pub const ASYNC_EVENT_CHECK_INTERVAL: zx::MonotonicDuration =
    zx::MonotonicDuration::from_seconds(1);

/// Returns `true` once the stream yields a `true`.
///
/// If the stream never yields `true` or never terminates, `try_any` may never resolve.
pub async fn try_any<S: Stream<Item = Result<bool>>>(stream: S) -> Result<bool> {
    let stream = pin!(stream);
    stream.try_filter(|v| futures::future::ready(*v)).next().await.unwrap_or(Ok(false))
}

/// Returns `true` if the stream only yields `true`.
///
/// If the stream never yields `false` or never terminates, `try_all` may never resolve.
pub async fn try_all<S: Stream<Item = Result<bool>>>(stream: S) -> Result<bool> {
    let stream = pin!(stream);
    stream.try_filter(|v| futures::future::ready(!*v)).next().await.unwrap_or(Ok(true))
}

/// Asynchronously sleeps for specified `secs` seconds.
pub async fn sleep(secs: i64) {
    fasync::Timer::new(zx::MonotonicDuration::from_seconds(secs).after_now()).await;
}

/// Gets a component event stream yielding component stopped events.
pub async fn get_component_stopped_event_stream() -> Result<component_events::events::EventStream> {
    EventStream::open_at_path("/events/stopped")
        .await
        .context("failed to subscribe to `Stopped` events")
}

/// Waits for a `stopped` event to be emitted for a component in a test realm.
///
/// Optionally specifies a matcher for the expected exit status of the `stopped`
/// event.
pub async fn wait_for_component_stopped_with_stream(
    event_stream: &mut component_events::events::EventStream,
    realm: &netemul::TestRealm<'_>,
    component_moniker: &str,
    status_matcher: Option<component_events::matcher::ExitStatusMatcher>,
) -> Result<component_events::events::Stopped> {
    let matcher = get_child_component_event_matcher(realm, component_moniker)
        .await
        .context("get child component matcher")?;
    matcher.stop(status_matcher).wait::<component_events::events::Stopped>(event_stream).await
}

/// Like [`wait_for_component_stopped_with_stream`] but retrieves an event
/// stream for the caller.
///
/// Note that this function fails to observe stop events that happen in early
/// realm creation, which is especially true for eager components.
pub async fn wait_for_component_stopped(
    realm: &netemul::TestRealm<'_>,
    component_moniker: &str,
    status_matcher: Option<component_events::matcher::ExitStatusMatcher>,
) -> Result<component_events::events::Stopped> {
    let mut stream = get_component_stopped_event_stream().await?;
    wait_for_component_stopped_with_stream(&mut stream, realm, component_moniker, status_matcher)
        .await
}

/// Gets an event matcher for `component_moniker` in `realm`.
pub async fn get_child_component_event_matcher(
    realm: &netemul::TestRealm<'_>,
    component_moniker: &str,
) -> Result<component_events::matcher::EventMatcher> {
    let realm_moniker = &realm.get_moniker().await.context("calling get moniker")?;
    let moniker_for_match =
        format!("./{}/{}/{}", NETEMUL_SANDBOX_MONIKER, realm_moniker, component_moniker);
    Ok(component_events::matcher::EventMatcher::ok().moniker(moniker_for_match))
}

/// The name of the netemul sandbox component, which is the parent component of
/// managed test realms.
const NETEMUL_SANDBOX_MONIKER: &str = "sandbox";

/// Gets the moniker of a component in a test realm, relative to the root of the
/// dynamic collection in which it is running.
pub async fn get_component_moniker<'a>(
    realm: &netemul::TestRealm<'a>,
    component: &str,
) -> Result<String> {
    let realm_moniker = realm.get_moniker().await.context("calling get moniker")?;
    Ok([NETEMUL_SANDBOX_MONIKER, &realm_moniker, component].join("/"))
}

/// Gets inspect data in realm.
///
/// Returns the resulting inspect data for `component` filtered by `tree_selector`.
pub async fn get_inspect_data(
    realm: &netemul::TestRealm<'_>,
    component_moniker: impl Into<String>,
    tree_selector: impl Into<String>,
) -> Result<diagnostics_hierarchy::DiagnosticsHierarchy> {
    let moniker = realm.get_moniker().await.context("calling get moniker")?;
    let realm_moniker = selectors::sanitize_string_for_selectors(&moniker);
    let mut data = diagnostics_reader::ArchiveReader::inspect()
        .retry(diagnostics_reader::RetryConfig::MinSchemaCount(1))
        .add_selector(
            diagnostics_reader::ComponentSelector::new(vec![
                NETEMUL_SANDBOX_MONIKER.into(),
                realm_moniker.into_owned(),
                component_moniker.into(),
            ])
            .with_tree_selector(tree_selector.into()),
        )
        .snapshot()
        .await
        .context("snapshot did not return any inspect data")?
        .into_iter()
        .map(|inspect_data| {
            inspect_data.payload.ok_or_else(|| {
                anyhow::anyhow!(
                    "empty inspect payload, metadata errors: {:?}",
                    inspect_data.metadata.errors
                )
            })
        });

    let Some(datum) = data.next() else {
        unreachable!("archive reader RetryConfig specifies non-empty")
    };

    let data: Vec<_> = data.collect();
    assert!(
        data.is_empty(),
        "expected a single inspect entry; got {:?} and also {:?}",
        datum,
        data
    );

    datum
}

/// Like [`get_inspect_data`] but returns a single property matched by
/// `property_selector`.
pub async fn get_inspect_property(
    realm: &netemul::TestRealm<'_>,
    component_moniker: impl Into<String>,
    property_selector: impl Into<String>,
) -> Result<diagnostics_hierarchy::Property> {
    let property_selector = property_selector.into();
    let hierarchy = get_inspect_data(&realm, component_moniker, property_selector.clone())
        .await
        .context("getting hierarchy")?;
    let property_selector = property_selector.split(&['/', ':']).skip(1).collect::<Vec<_>>();
    let property = hierarchy
        .get_property_by_path(&property_selector)
        .ok_or_else(|| anyhow::anyhow!("property not found in hierarchy: {hierarchy:?}"))?;
    Ok(property.clone())
}

/// Read an Inspect hierarchy and filter it down to properties of interest from the diagnostics
/// directory of Netstack2. For any other component, please use `get_inspect_data`, this function
/// doesn't apply to any other component and won't work.
// TODO(https://fxbug.dev/324494668): remove when Netstack2 is gone.
pub async fn get_deprecated_netstack2_inspect_data(
    diagnostics_dir: &fio::DirectoryProxy,
    subdir: &str,
    selectors: impl IntoIterator<Item = Selector>,
) -> DiagnosticsHierarchy {
    let matcher = HierarchyMatcher::new(selectors.into_iter()).expect("invalid selectors");
    loop {
        // NOTE: For current test purposes we just need to read from the deprecated inspect
        // protocol. If this changes in the future, then we'll need to update this code to be able
        // to read from other kind-of files such as fuchsia.inspect.Tree or a *.inspect VMO file.
        let proxy = client::connect_to_named_protocol_at_dir_root::<InspectMarker>(
            diagnostics_dir,
            &format!("{subdir}/{}", InspectMarker::PROTOCOL_NAME),
        )
        .unwrap();
        match inspect_fidl_load::load_hierarchy(proxy).await {
            Ok(hierarchy) => return filter_hierarchy(hierarchy, &matcher).unwrap(),
            Err(err) => {
                println!("Failed to load hierarchy, retrying. Error: {err:?}")
            }
        }
        fasync::Timer::new(fasync::MonotonicDuration::from_millis(100)).await;
    }
}

/// Sets up a realm with a network with no required services.
pub async fn setup_network<'a, N: realms::Netstack>(
    sandbox: &'a netemul::TestSandbox,
    name: &'a str,
    metric: Option<u32>,
) -> Result<(
    netemul::TestNetwork<'a>,
    netemul::TestRealm<'a>,
    netemul::TestInterface<'a>,
    netemul::TestFakeEndpoint<'a>,
)> {
    setup_network_with::<N, _>(
        sandbox,
        name,
        netemul::InterfaceConfig { metric, ..Default::default() },
        std::iter::empty::<fnetemul::ChildDef>(),
    )
    .await
}

/// Sets up a realm with required services and a network used for tests
/// requiring manual packet inspection and transmission.
///
/// Returns the network, realm, netstack client, interface (added to the
/// netstack and up) and a fake endpoint used to read and write raw ethernet
/// packets.
pub async fn setup_network_with<'a, N: realms::Netstack, I>(
    sandbox: &'a netemul::TestSandbox,
    name: &'a str,
    interface_config: netemul::InterfaceConfig<'a>,
    children: I,
) -> Result<(
    netemul::TestNetwork<'a>,
    netemul::TestRealm<'a>,
    netemul::TestInterface<'a>,
    netemul::TestFakeEndpoint<'a>,
)>
where
    I: IntoIterator,
    I::Item: Into<fnetemul::ChildDef>,
{
    let network = sandbox.create_network(name).await.context("failed to create network")?;
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(name, children)
        .context("failed to create netstack realm")?;
    // It is important that we create the fake endpoint before we join the
    // network so no frames transmitted by Netstack are lost.
    let fake_ep = network.create_fake_endpoint()?;

    let iface = realm
        .join_network_with_if_config(&network, name, interface_config)
        .await
        .context("failed to configure networking")?;

    Ok((network, realm, iface, fake_ep))
}

/// Pauses the fake clock in the given realm.
pub async fn pause_fake_clock(realm: &netemul::TestRealm<'_>) -> Result<()> {
    let fake_clock_control = realm
        .connect_to_protocol::<fidl_fuchsia_testing::FakeClockControlMarker>()
        .context("failed to connect to FakeClockControl")?;
    fake_clock_control.pause().await.context("failed to pause time")?;
    Ok(())
}

/// Wraps `fut` so that it prints `event_name` and the caller's location to
/// stderr every `interval` until `fut` completes.
#[track_caller]
pub fn annotate<'a, 'b: 'a, T>(
    fut: impl Future<Output = T> + 'a,
    interval: std::time::Duration,
    event_name: &'b str,
) -> impl Future<Output = T> + 'a {
    let caller = std::panic::Location::caller();

    async move {
        let mut fut = pin!(fut.fuse());
        let event_name = event_name.to_string();
        let mut print_fut = pin!(
            futures::stream::repeat(())
                .for_each(|()| async {
                    fasync::Timer::new(interval).await;
                    eprintln!("waiting for {} at {}", event_name, caller);
                })
                .fuse()
        );
        let result = select! {
            result = fut => result,
            () = print_fut => unreachable!("should repeat printing forever"),
        };
        eprintln!("completed {} at {}", event_name, caller);
        result
    }
}
