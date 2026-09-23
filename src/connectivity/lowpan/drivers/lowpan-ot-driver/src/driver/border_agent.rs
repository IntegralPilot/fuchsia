// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(dead_code)]

use super::*;
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_net_mdns::*;
use fuchsia_component::client::connect_to_protocol;
use futures::channel::mpsc;
use futures::future::Fuse;
use futures::select;
use openthread::ot::BorderAgentEphemeralKeyState;
use rand;
use std::pin::pin;

const BORDER_AGENT_SERVICE_TYPE: &str = "_meshcop._udp.";
const BORDER_AGENT_EPSKC_SERVICE_TYPE: &str = "_meshcop-e._udp.";

// Port 9 is the old-school discard port.
const BORDER_AGENT_SERVICE_PLACEHOLDER_PORT: u16 = 9;

// Number of times to attempt to publish services.
const MAX_PUBLISH_SERVICE_ATTEMPTS: usize = 2;

// Number of times to attempt to check for mDNS service removal.
const MAX_MDNS_SERVICE_REMOVAL_CHECKS: usize = 2;

// The interval to wait before re-checking mDNS service removal.
const MDNS_SERVICE_REMOVAL_CHECK_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(250);

// These flags are ultimately defined by table 8-5 of the Thread v1.1.1 specification.
// Additional flags originate from the source code found [here][1].
//
// [1]: https://github.com/openthread/ot-br-posix/blob/36db8891576a6ed571ad319afca734c5288c4cd9/src/border_agent/border_agent.cpp#L86
bitflags::bitflags! {
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BorderAgentState : u32 {
    const CONNECTION_MODE_PSKC = 1;
    const CONNECTION_MODE_PSKD = 2;
    const CONNECTION_MODE_VENDOR = 3;
    const CONNECTION_MODE_X509 = 4;
    const THREAD_IF_STATUS_INITIALIZED = (1<<3);
    const THREAD_IF_STATUS_ACTIVE = (2<<3);
    const HIGH_AVAILABILITY = (1<<5);
    const BBR_IS_ACTIVE = (1<<7);
    const BBR_IS_PRIMARY = (1<<8);
    const EPSKC_SUPPORTED = (1<<11);
}
}

#[derive(Debug)]
pub struct BorderAgent {
    pub update_sender: fuchsia_sync::Mutex<Option<futures::channel::mpsc::UnboundedSender<()>>>,
    pub update_receiver: fuchsia_sync::Mutex<Option<futures::channel::mpsc::UnboundedReceiver<()>>>,
}

impl BorderAgent {
    pub fn new() -> Self {
        BorderAgent {
            update_sender: fuchsia_sync::Mutex::new(None),
            update_receiver: fuchsia_sync::Mutex::new(None),
        }
    }

    /// Trigger a border agent service update from here.
    pub fn trigger_service_update(&self) {
        if let Some(ref sender) = *self.update_sender.lock() {
            let _ = sender.unbounded_send(());
        }
    }
}

impl Default for BorderAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct Epskc {
    pub update_sender: fuchsia_sync::Mutex<Option<futures::channel::mpsc::UnboundedSender<()>>>,
    pub update_receiver: fuchsia_sync::Mutex<Option<futures::channel::mpsc::UnboundedReceiver<()>>>,
}

impl Epskc {
    pub fn new() -> Self {
        Epskc {
            update_sender: fuchsia_sync::Mutex::new(None),
            update_receiver: fuchsia_sync::Mutex::new(None),
        }
    }

    /// Trigger a ePSKc mode state update from here.
    pub fn trigger_service_update(&self) {
        if let Some(ref sender) = *self.update_sender.lock() {
            let _ = sender.unbounded_send(());
        }
    }
}

impl Default for Epskc {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_openthread_txt_data(txt_data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut result = Vec::new();
    let mut offset = 0;

    while offset < txt_data.len() {
        let len = txt_data[offset] as usize;
        offset += 1;

        if offset + len > txt_data.len() {
            break;
        }

        let record = &txt_data[offset..offset + len];
        offset += len;

        if let Some(eq_pos) = record.iter().position(|&b| b == b'=') {
            let key = String::from_utf8_lossy(&record[..eq_pos]).to_string();
            let value = record[eq_pos + 1..].to_vec();
            result.push((key, value));
        } else if !record.is_empty() {
            let key = String::from_utf8_lossy(record).to_string();
            result.push((key, Vec::new()));
        }
    }

    result
}

fn get_openthread_meshcop_txt_data<OT>(ot_instance: &OT) -> Vec<(String, Vec<u8>)>
where
    OT: ot::InstanceInterface,
{
    // Get TXT data from OpenThread
    match ot_instance.border_agent_get_meshcop_service_txt_data() {
        Ok(ot_txt_data) => {
            debug!(tag = "meshcop"; "Using OpenThread core TXT data, {} bytes", ot_txt_data.len());
            parse_openthread_txt_data(&ot_txt_data)
        }
        Err(err) => {
            warn!(tag = "meshcop"; "Failed to get OpenThread TXT data: {:?}", err);
            Vec::new()
        }
    }
}

fn get_vendor_meshcop_txt_data(vendor: &str, product: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        ("vn".to_string(), vendor.as_bytes().to_vec()),
        ("mn".to_string(), product.as_bytes().to_vec()),
    ]
}

fn calc_meshcop_service_txt<OT>(
    ot_instance: &OT,
    vendor: &str,
    product: &str,
) -> Vec<(String, Vec<u8>)>
where
    OT: ot::InstanceInterface,
{
    let mut txt = get_openthread_meshcop_txt_data(ot_instance);
    txt.extend(get_vendor_meshcop_txt_data(vendor, product));
    txt
}

enum PublishServiceFailure {
    PublishInstanceRequestFailure,
    PublishInstanceFailure,
    AlreadyPublishedLocally,
    ResponderFailure,
}

async fn publish_service(
    tag: &str,
    service_type: &str,
    service_instance: &str,
    txt: Option<Vec<(String, Vec<u8>)>>,
    port: u16,
    publisher: ServiceInstancePublisherProxy,
) -> Result<(), PublishServiceFailure> {
    let (client, server) = create_endpoints::<ServiceInstancePublicationResponder_Marker>();
    let publish_init_future = publisher
        .publish_service_instance(
            service_type,
            service_instance,
            &ServiceInstancePublicationOptions::default(),
            client,
        )
        .map(|x| -> Result<(), PublishServiceFailure> {
            match x {
                Ok(Ok(())) => Ok(()),
                Ok(Err(
                    fidl_fuchsia_net_mdns::PublishServiceInstanceError::AlreadyPublishedLocally,
                )) => Err(PublishServiceFailure::AlreadyPublishedLocally),
                Ok(Err(err)) => {
                    error!(tag = tag; "publish_init_future failed: {:?}", err);
                    Err(PublishServiceFailure::PublishInstanceFailure)
                }
                Err(zx_err) => {
                    error!(tag = tag; "publish_init_future failed: {:?}", zx_err);
                    Err(PublishServiceFailure::PublishInstanceRequestFailure)
                }
            }
        });

    // Prepare our static response for all queries.
    let publication = ServiceInstancePublication {
        port: Some(port),
        text: txt.map(|txt| {
            txt.iter()
                .map(|(key, value)| {
                    let mut x = key.as_bytes().to_vec();
                    x.push(b'=');
                    x.extend(value.as_slice());
                    x
                })
                .collect::<Vec<_>>()
        }),
        ..Default::default()
    };

    let publish_responder_future = server.into_stream().map_err(Into::into).try_for_each(
        move |ServiceInstancePublicationResponder_Request::OnPublication {
                  publication_cause,
                  subtype,
                  source_addresses,
                  responder,
              }| {
            debug!(
                tag = tag;
                "publication_cause: {publication_cause:?}"
            );
            debug!(tag = tag; "publication_cause: {subtype:?}");
            debug!(
                tag = tag;
                "source_addresses: {source_addresses:?}"
            );
            debug!(
                tag = tag;
                "publication: {:?}", publication
            );

            // Due to https://fxbug.dev/42182233, the publication responder channel will close
            // if the publisher that created it is closed.
            // TODO(https://fxbug.dev/42182233): Remove this line once https://fxbug.dev/42182233 is fixed.
            let _publisher = publisher.clone();

            let result = if subtype.is_some() {
                debug!(
                    tag = tag;
                    "Subtype specified, skipping advertisement."
                );
                Err(OnPublicationError::DoNotRespond)
            } else {
                Ok(&publication)
            };

            futures::future::ready(
                responder.send(result).context("Unable to call publication responder"),
            )
        },
    );

    futures::try_join!(
        publish_init_future,
        publish_responder_future.map_err(|err| {
            error!(tag = tag; "publish_responder_future failed: {:?}", err);
            PublishServiceFailure::ResponderFailure
        }),
    )?;

    Ok(())
}

async fn publish_border_agent_service(
    service_instance: String,
    txt: Vec<(String, Vec<u8>)>,
    port: u16,
    publisher: ServiceInstancePublisherProxy,
    subscriber: Option<ServiceSubscriber2Proxy>,
) -> Result<(), anyhow::Error> {
    let tag = "meshcop";
    let mut current_instance_name = service_instance;

    for _ in 0..MAX_PUBLISH_SERVICE_ATTEMPTS {
        let mut publish_success = false;

        // Try publishing with current_instance_name, waiting up to
        // MAX_MDNS_SERVICE_REMOVAL_CHECKS times. If AlreadyPublishedLocally is
        // encountered, wait for MDNS_SERVICE_REMOVAL_CHECK_INTERVAL for the
        // service to be removed.
        for wait_attempt in 0..MAX_MDNS_SERVICE_REMOVAL_CHECKS {
            match publish_service(
                tag,
                BORDER_AGENT_SERVICE_TYPE,
                &current_instance_name,
                Some(txt.clone()),
                port,
                publisher.clone(),
            )
            .await
            {
                Ok(()) => {
                    publish_success = true;
                    break;
                }
                Err(PublishServiceFailure::AlreadyPublishedLocally) => {
                    warn!(tag; "Service {:?} already published locally. wait_attempt: {}",
                        current_instance_name, wait_attempt);
                    // Try to wait for 0.25 seconds for the service to be removed.
                    // Assumed that we have removed the service earlier and mdns stack was not
                    // properly procseed it yet.
                    let timeout = fuchsia_async::Timer::new(MDNS_SERVICE_REMOVAL_CHECK_INTERVAL);
                    let wait_fut = wait_for_service_removal(
                        BORDER_AGENT_SERVICE_TYPE,
                        &current_instance_name,
                        subscriber.clone(),
                    );

                    let result = match futures::future::select(
                        std::pin::pin!(timeout),
                        std::pin::pin!(wait_fut),
                    )
                    .await
                    {
                        futures::future::Either::Left(_) => {
                            Err(anyhow::anyhow!("Timeout waiting for service removal"))
                        }
                        futures::future::Either::Right((res, _)) => res,
                    };
                    if let Err(e) = result {
                        warn!(tag; "Error waiting for mDNS service removal: {:?}", e);
                    }
                }
                Err(PublishServiceFailure::PublishInstanceRequestFailure) => {
                    return Err(anyhow::format_err!("Failed to publish meshcop service."));
                }
                Err(PublishServiceFailure::PublishInstanceFailure)
                | Err(PublishServiceFailure::ResponderFailure) => {
                    warn!(tag; "Publish attempt failed.");
                    break;
                }
            }
        }

        if publish_success {
            return Ok(());
        }

        // If we didn't succeed, we calculate an alternative name and retry the outer loop.
        current_instance_name = get_alternate_service_instance_name(&current_instance_name);
    }

    Err(anyhow::format_err!("Exhausted service publication retry attempts."))
}

pub(crate) enum BorderAgentPublishRequest {
    Stop,
    Start { port: u16, txt: Vec<(String, Vec<u8>)>, instance_name: String },
}

async fn wait_for_service_removal(
    service_type: impl Into<String>,
    instance_name: impl Into<String>,
    subscriber: Option<ServiceSubscriber2Proxy>,
) -> Result<(), anyhow::Error> {
    let service_type = service_type.into();
    let instance_name = instance_name.into();

    let subscriber = if let Some(sub) = subscriber {
        sub
    } else {
        connect_to_protocol::<ServiceSubscriber2Marker>()?
    };
    let (client, server) = create_endpoints::<ServiceSubscriptionListenerMarker>();

    subscriber.subscribe_to_service(
        &service_type,
        &ServiceSubscriptionOptions {
            exclude_local: Some(false),
            exclude_local_proxies: Some(false),
            ..Default::default()
        },
        client,
    )?;

    let mut stream = server.into_stream();

    while let Some(request) = stream.try_next().await? {
        match request {
            ServiceSubscriptionListenerRequest::OnInstanceLost { instance, responder, .. } => {
                responder.send().context("Failed to send OnInstanceLost response")?;
                if instance == instance_name {
                    return Ok(());
                }
            }
            ServiceSubscriptionListenerRequest::OnInstanceDiscovered { responder, .. } => {
                responder.send().context("Failed to send OnInstanceDiscovered response")?;
            }
            ServiceSubscriptionListenerRequest::OnInstanceChanged { responder, .. } => {
                responder.send().context("Failed to send OnInstanceChanged response")?;
            }
            ServiceSubscriptionListenerRequest::OnQuery { responder, .. } => {
                responder.send().context("Failed to send OnQuery response")?;
            }
        }
    }
    anyhow::bail!("Service subscription stream ended before removal of {:?}", instance_name)
}

pub(crate) async fn manage_border_agent_service_publisher(
    mut receiver: mpsc::Receiver<BorderAgentPublishRequest>,
    publisher: ServiceInstancePublisherProxy,
) -> Result<(), anyhow::Error> {
    let publication_fut = Fuse::terminated();
    let mut publication_fut = pin!(publication_fut);

    loop {
        select! {
             result = publication_fut => {
                 info!("meshcop publication completed: {result:?}");
             }
             state = receiver.select_next_some() => {
                 match state {
                    BorderAgentPublishRequest::Start { port, txt, instance_name } => {
                         let cloned_publisher = publisher.clone();
                         publication_fut.set(async move {
                             let _ = publish_border_agent_service(
                                 instance_name,
                                 txt,
                                 port,
                                 cloned_publisher,
                                 None,
                             ).await;
                         }.fuse());
                    }
                    BorderAgentPublishRequest::Stop => {
                         publication_fut.set(Fuse::terminated());
                    }
                }
            }
            complete => {
                error!("meshcop receiver completed unexpectedly");
                return Err(anyhow::format_err!("meshcop receiver completed unexpectedly"))
            }
        }
    }
}

pub(crate) enum PublishServiceRequest {
    Stop,
    Start { port: u16, service_instance: String },
}

async fn publish_epskc_service(
    service_instance: String,
    port: u16,
    publisher: ServiceInstancePublisherProxy,
) -> Result<(), anyhow::Error> {
    let tag = "meshcop-e";

    for i in 0..MAX_PUBLISH_SERVICE_ATTEMPTS {
        let service_name = match i {
            0 => service_instance.clone(),
            _ => get_alternate_service_instance_name(service_instance.as_str()),
        };

        match publish_service(
            tag,
            BORDER_AGENT_EPSKC_SERVICE_TYPE,
            service_name.as_str(),
            None,
            port,
            publisher.clone(),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(PublishServiceFailure::AlreadyPublishedLocally) => {
                warn!(tag; "Service {:?} already published locally.", service_name);
            }
            Err(PublishServiceFailure::PublishInstanceRequestFailure) => {
                return Err(anyhow::format_err!("Failed to publish ePSKc service."));
            }
            Err(PublishServiceFailure::PublishInstanceFailure)
            | Err(PublishServiceFailure::ResponderFailure) => {
                warn!(tag; "Publish attempt failed.",);
            }
        }
    }

    Err(anyhow::format_err!("Exhausted service publication retry attempts."))
}

pub(crate) async fn manage_epskc_service_publisher(
    mut receiver: mpsc::Receiver<PublishServiceRequest>,
    publisher: ServiceInstancePublisherProxy,
) -> Result<(), anyhow::Error> {
    let publication_fut = Fuse::terminated();
    let mut publication_fut = pin!(publication_fut);

    loop {
        select! {
            result = publication_fut => {
                info!("ePSKc publication completed: {result:?}");
            }
            state = receiver.select_next_some() => {
                match state {
                    PublishServiceRequest::Start {port, service_instance} => {
                        // Begin publishing the new service description.  Any existing publication
                        // action will be dropped.
                        let cloned_publisher = publisher.clone();
                        publication_fut.set(async move {
                            publish_epskc_service(
                            service_instance,
                            port,
                            cloned_publisher,
                        ).await
                        }.fuse());
                    }
                    PublishServiceRequest::Stop => {
                        // Set the publication future to a terminated future to stop any ongoing
                        // publication.
                        publication_fut.set(Fuse::terminated());
                    }
                }
            }
        }
    }
}

// Functional equivalent to ot-br-posix's BorderAgent::GetServiceInstanceNameWithExtAddr.
// Following the Thread specification and simplifying service identification the DNS-SD
// service instance name used by the Border Agent (e.g., for meshcop and meshcop-e) is
// generated using the pattern: "vendor name + product name + #XYZW + service type", the
// "XYZW" represents the last two bytes of the extended address, in uppercase hexadecimal.
pub(crate) fn get_service_instance_name_with_ext_addr(
    vendor: &str,
    product: &str,
    ext_addr: &[u8],
) -> String {
    format!("{} {} #{}", vendor, product, hex::encode(&ext_addr[6..]).to_uppercase())
}

fn get_alternate_service_instance_name(base_instance_name: &str) -> String {
    let rand_u16 = rand::random::<u16>();
    format!("{} ({})", base_instance_name, rand_u16)
}

impl<OT: ot::InstanceInterface, NI, BI> OtDriver<OT, NI, BI> {
    pub async fn update_border_agent_service(&self) {
        let vendor = self.product_metadata.vendor();
        let product = self.product_metadata.product();
        let service_instance_name = {
            let driver_state = self.driver_state.lock();
            let ot_instance = &driver_state.ot_instance;
            get_service_instance_name_with_ext_addr(
                &vendor,
                &product,
                &ot_instance.get_extended_address().as_slice(),
            )
        };

        let (txt, port) = {
            let mut txt = self.border_agent_vendor_txt_entries.lock().await.clone();

            let driver_state = self.driver_state.lock();
            let ot_instance = &driver_state.ot_instance;
            for (key, value) in calc_meshcop_service_txt(ot_instance, &vendor, &product) {
                if !txt.iter().any(|(k, _)| k == &key) {
                    txt.push((key, value));
                }
            }
            let port = if ot_instance.border_agent_is_active() == true {
                ot_instance.border_agent_get_udp_port()
            } else {
                // The following comment is from the original ot-br-posix implementation:
                // ---
                // When thread interface is not active, the border agent is not started,
                // thus it's not listening to any port and not handling requests. In such
                // situation, we use a placeholder port number for publishing the MeshCoP
                // service to advertise the status of the border router. One can learn
                // the thread interface status from `sb` entry so it doesn't have to send
                // requests to the placeholder port when border agent is not running.
                BORDER_AGENT_SERVICE_PLACEHOLDER_PORT
            };
            (txt, port)
        };

        let border_agent_current_txt_entries = self.border_agent_current_txt_entries.clone();
        let mut last_txt_entries = border_agent_current_txt_entries.lock().await;

        if txt == *last_txt_entries {
            debug!(tag = "meshcop"; "update_border_agent_service: No changes.");
        } else {
            debug!(
                tag = "meshcop";
                "update_border_agent_service: Updating meshcop dns-sd: port={} txt=[PII]({:?})",
                port,
                txt
            );

            *last_txt_entries = txt.clone();

            let mut sender = self.border_agent_publisher.lock();
            if let Err(e) = sender.try_send(BorderAgentPublishRequest::Start {
                port,
                txt,
                instance_name: service_instance_name,
            }) {
                warn!("Could not post Border Agent Start event: {:?}", e);
            }
        }
    }

    pub async fn handle_epskc_state_changed(&self) {
        // Get all of the state information from OT that requires locking on OtDriver fields.
        // The current ePSKc state determines what action will be taken if any.  If the service has
        // been started, the extended address is used in deriving the service instance name and the
        // port is published to listeners.
        let (state, port, ext_addr) = {
            let driver_state = self.driver_state.lock();
            let ot_instance = &driver_state.ot_instance;
            let state = ot_instance.border_agent_ephemeral_key_get_state();
            let port = ot_instance.border_agent_ephemeral_key_get_udp_port();
            let ext_addr = ot_instance.get_extended_address().as_slice().to_vec();

            (state, port, ext_addr)
        };

        let mut epskc_publisher = self.epskc_publisher.lock();

        info!(
            tag = "meshcop-e"; "handle_epskc_state_changed: handling state change {:?}", state
        );
        match state {
            BorderAgentEphemeralKeyState::Started => {
                // Started: "Ephemeral key is set. Listening to accept secure connections."
                //
                // When a new ephemeral key is set, stop any existing publication and publish the
                // latest information.

                // Derive the service name.
                let vendor = self.product_metadata.vendor();
                let product = self.product_metadata.product();
                let service_instance =
                    get_service_instance_name_with_ext_addr(&vendor, &product, ext_addr.as_slice());

                if let Err(e) = epskc_publisher
                    .try_send(PublishServiceRequest::Start { port, service_instance })
                {
                    warn!("Could not post Start event: {e:}")
                };
            }
            BorderAgentEphemeralKeyState::Disabled | BorderAgentEphemeralKeyState::Stopped => {
                // Disabled: "Ephemeral Key Manager is disabled."
                // Stopped: "Enabled, but no ephemeral key is in use (not set or started)."
                //
                // If the ePSKc feature is disabled or no ephemeral key is available for use, ensure
                // that the service is no longer published.
                if let Err(e) = epskc_publisher.try_send(PublishServiceRequest::Stop) {
                    warn!("Could not post Stop event: {e:}")
                }
            }
            BorderAgentEphemeralKeyState::Connected | BorderAgentEphemeralKeyState::Accepted => {
                // Here is the documentation associated with these states:
                //
                // Connected: "Session is established with an external commissioner candidate."
                // Accepted: "Session is established and candidate is accepted as full commissioner."
                //
                // These state transitions are internal to OpenThread.  No action is required from
                // the platform as the ePSKc service must already be actively published in order for
                // these states to occur.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::{Proxy, create_proxy};
    use fuchsia_async::TestExecutor;

    use std::task::Poll;
    use {fidl_fuchsia_net_mdns as fidl_mdns, regex};

    const TEST_PORT: u16 = 1234;
    const TEST_SERVICE: &str = "test test test";

    use std::sync::LazyLock;

    static TEST_TEXT: LazyLock<Vec<(String, Vec<u8>)>> = LazyLock::new(|| {
        vec![(String::from("abcd"), vec![1, 2, 3, 4]), (String::from("wxyz"), vec![5, 6, 7, 8])]
    });

    struct TestValues {
        publisher: ServiceInstancePublisherProxy,
        publisher_stream: ServiceInstancePublisherRequestStream,
        sender: mpsc::Sender<PublishServiceRequest>,
        receiver: mpsc::Receiver<PublishServiceRequest>,
    }

    fn test_setup() -> TestValues {
        let (publisher, publisher_reqs) =
            create_proxy::<fidl_mdns::ServiceInstancePublisherMarker>();
        let publisher_stream = publisher_reqs.into_stream();

        let (sender, receiver) = mpsc::channel(100);

        return TestValues { publisher, publisher_stream, sender, receiver };
    }

    #[fuchsia::test]
    fn test_publish_border_agent_service_publication_failure() {
        let mut exec = TestExecutor::new_with_fake_time();
        let mut test_vals = test_setup();

        let (subscriber, subscriber_reqs) = create_proxy::<ServiceSubscriber2Marker>();
        let mut subscriber_stream = subscriber_reqs.into_stream();

        let fut = publish_border_agent_service(
            String::from(TEST_SERVICE),
            TEST_TEXT.to_vec(),
            TEST_PORT,
            test_vals.publisher,
            Some(subscriber),
        );
        let mut fut = pin!(fut);

        for i in 0..MAX_PUBLISH_SERVICE_ATTEMPTS {
            // First loop attempt (wait_attempt: 0).
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            assert_matches!(
                exec.run_until_stalled(&mut test_vals.publisher_stream.next()),
                Poll::Ready(Some(Ok(fidl_mdns::ServiceInstancePublisherRequest::PublishServiceInstance {
                    service, instance, options, publication_responder: _, responder,
                }))) => {
                        match i {
                            0 => assert_eq!(instance, TEST_SERVICE),
                            _ => {
                                let re = regex::Regex::new(
                                    (TEST_SERVICE.to_owned() + " \\([0-9]+\\)").as_str()
                                ).unwrap();
                                assert!(re.is_match(&instance))
                            }
                        }
                    assert_eq!(service, BORDER_AGENT_SERVICE_TYPE);
                    assert_eq!(options, ServiceInstancePublicationOptions::default());
                    responder
                        .send(Err(fidl_mdns::PublishServiceInstanceError::AlreadyPublishedLocally))
                        .expect("Failed to send publish response");
                }
            );

            // Step future to let the timeout register.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            assert_matches!(
                exec.run_until_stalled(&mut subscriber_stream.next()),
                Poll::Ready(Some(Ok(ServiceSubscriber2Request::SubscribeToService { .. })))
            );

            // Wait 0.5s timeout logic (wait_attempt: 0)
            let _ = exec.wake_next_timer();

            // Second loop attempt (wait_attempt: 1).
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
            assert_matches!(
                exec.run_until_stalled(&mut test_vals.publisher_stream.next()),
                Poll::Ready(Some(Ok(fidl_mdns::ServiceInstancePublisherRequest::PublishServiceInstance {
                    service, instance, options, publication_responder: _, responder,
                }))) => {
                        match i {
                            0 => assert_eq!(instance, TEST_SERVICE),
                            _ => {
                                let re = regex::Regex::new(
                                    (TEST_SERVICE.to_owned() + " \\([0-9]+\\)").as_str()
                                ).unwrap();
                                assert!(re.is_match(&instance))
                            }
                        }
                    assert_eq!(service, BORDER_AGENT_SERVICE_TYPE);
                    assert_eq!(options, ServiceInstancePublicationOptions::default());
                    responder
                        .send(Err(fidl_mdns::PublishServiceInstanceError::AlreadyPublishedLocally))
                        .expect("Failed to send publish response");
                }
            );

            // Step future to let the timeout register.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            assert_matches!(
                exec.run_until_stalled(&mut subscriber_stream.next()),
                Poll::Ready(Some(Ok(ServiceSubscriber2Request::SubscribeToService { .. })))
            );

            // Wait 0.5s timeout logic (wait_attempt: 1).
            let _ = exec.wake_next_timer();
        }

        // The future should complete with an error.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_epskc_service_publication_failure() {
        let mut exec = TestExecutor::new();
        let mut test_vals = test_setup();

        let fut = publish_epskc_service(String::from(TEST_SERVICE), TEST_PORT, test_vals.publisher);
        let mut fut = pin!(fut);

        for i in 0..MAX_PUBLISH_SERVICE_ATTEMPTS {
            // Progress the future and expect it to stall while attempting to interact with MDNS.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // There should now be a request from the publisher.
            assert_matches!(
                exec.run_until_stalled(&mut test_vals.publisher_stream.next()),
                Poll::Ready(Some(Ok(fidl_mdns::ServiceInstancePublisherRequest::PublishServiceInstance {
                    service, instance, options, publication_responder: _, responder,
                }))) => {
                        match i {
                            0 => assert_eq!(instance, TEST_SERVICE),
                            _ => {
                                let re = regex::Regex::new(
                                    (TEST_SERVICE.to_owned() + " \\([0-9]+\\)").as_str()
                                ).unwrap();
                                assert!(re.is_match(&instance))
                            }
                        }
                    assert_eq!(service, BORDER_AGENT_EPSKC_SERVICE_TYPE);
                    assert_eq!(options, ServiceInstancePublicationOptions::default());
                    responder
                        .send(Err(fidl_mdns::PublishServiceInstanceError::AlreadyPublishedLocally))
                        .expect("Failed to send publish response");
                }
            );
        }

        // The future should complete with an error.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_publish_border_agent_service_exits_when_publication_service_drops() {
        let mut exec = TestExecutor::new();
        let test_vals = test_setup();

        // Drop the publisher stream.
        drop(test_vals.publisher_stream);

        let fut = publish_border_agent_service(
            String::from(TEST_SERVICE),
            TEST_TEXT.to_vec(),
            TEST_PORT,
            test_vals.publisher,
            None,
        );
        let mut fut = pin!(fut);

        // Progress the future and expect it to stall while attempting to interact with MDNS.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_publish_epskc_service_exits_when_publication_service_drops() {
        let mut exec = TestExecutor::new();
        let test_vals = test_setup();

        // Drop the publisher stream.
        drop(test_vals.publisher_stream);

        let fut = publish_epskc_service(String::from(TEST_SERVICE), TEST_PORT, test_vals.publisher);
        let mut fut = pin!(fut);

        // Progress the future and expect it to stall while attempting to interact with MDNS.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_publish_border_agent_service_publication_success() {
        let mut exec = TestExecutor::new();
        let mut test_vals = test_setup();

        let fut = publish_border_agent_service(
            String::from(TEST_SERVICE),
            TEST_TEXT.to_vec(),
            TEST_PORT,
            test_vals.publisher,
            None,
        );
        let mut fut = pin!(fut);

        // Progress the future and expect it to stall while attempting to interact with MDNS.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // There should now be a request from the publisher.
        let responder = match exec.run_until_stalled(&mut test_vals.publisher_stream.next()) {
            Poll::Ready(Some(Ok(
                fidl_mdns::ServiceInstancePublisherRequest::PublishServiceInstance {
                    service,
                    instance,
                    options,
                    publication_responder,
                    responder,
                },
            ))) => {
                assert_eq!(service, BORDER_AGENT_SERVICE_TYPE);
                assert_eq!(instance, TEST_SERVICE);
                assert_eq!(options, ServiceInstancePublicationOptions::default());
                responder.send(Ok(())).expect("Failed to send publish response");

                publication_responder
            }
            other => panic!("Unexpected variant: {:?}", other),
        };
        let responder = responder.into_proxy();

        // The future should still be running.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Publish
        let publication_fut =
            responder.on_publication(ServiceInstancePublicationCause::Announcement, None, &[]);
        let mut publication_fut = pin!(publication_fut);
        assert_matches!(exec.run_until_stalled(&mut publication_fut), Poll::Pending);

        // Run the border agent future and observe:
        // 1. The border agent future is still running.
        // 2. The publication future completes.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut publication_fut),
            Poll::Ready(Ok(Ok(ServiceInstancePublication { port, text, .. }))) => {
                assert_eq!(port, Some(TEST_PORT));
                assert_eq!(text, Some(vec![
                    // asdf=1234
                    vec![97, 98, 99, 100, 61, 1, 2, 3, 4],
                    // wxyz=5678
                    vec![119, 120, 121, 122, 61, 5, 6, 7, 8],
                ]))
            }
        );

        // Verify that the publisher proxy is still held.
        assert_matches!(
            exec.run_until_stalled(&mut test_vals.publisher_stream.next()),
            Poll::Pending
        );
    }

    #[fuchsia::test]
    fn test_publish_espkc_service_publication_success() {
        let mut exec = TestExecutor::new();
        let mut test_vals = test_setup();

        let fut = publish_epskc_service(String::from(TEST_SERVICE), TEST_PORT, test_vals.publisher);
        let mut fut = pin!(fut);

        // Progress the future and expect it to stall while attempting to interact with MDNS.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // There should now be a request from the publisher.
        let responder = match exec.run_until_stalled(&mut test_vals.publisher_stream.next()) {
            Poll::Ready(Some(Ok(
                fidl_mdns::ServiceInstancePublisherRequest::PublishServiceInstance {
                    service,
                    instance,
                    options,
                    publication_responder,
                    responder,
                },
            ))) => {
                assert_eq!(service, BORDER_AGENT_EPSKC_SERVICE_TYPE);
                assert_eq!(instance, TEST_SERVICE);
                assert_eq!(options, ServiceInstancePublicationOptions::default());
                responder.send(Ok(())).expect("Failed to send publish response");

                publication_responder
            }
            other => panic!("Unexpected variant: {:?}", other),
        };
        let responder = responder.into_proxy();

        // The future should still be running.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Publish
        let publication_fut =
            responder.on_publication(ServiceInstancePublicationCause::Announcement, None, &[]);
        let mut publication_fut = pin!(publication_fut);
        assert_matches!(exec.run_until_stalled(&mut publication_fut), Poll::Pending);

        // Run the border agent future and observe:
        // 1. The border agent future is still running.
        // 2. The publication future completes.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut publication_fut),
            Poll::Ready(Ok(Ok(ServiceInstancePublication { port, .. }))) => {
                assert_eq!(port, Some(TEST_PORT));
            }
        );

        // Verify that the publisher proxy is still held.
        assert_matches!(
            exec.run_until_stalled(&mut test_vals.publisher_stream.next()),
            Poll::Pending
        );
    }

    #[fuchsia::test]
    fn test_publish_border_agent_service_exits_when_publisher_drops_responder() {
        let mut exec = TestExecutor::new();
        let mut test_vals = test_setup();

        let fut = publish_border_agent_service(
            String::from(TEST_SERVICE),
            TEST_TEXT.to_vec(),
            TEST_PORT,
            test_vals.publisher,
            None,
        );
        let mut fut = pin!(fut);

        // Progress the future and expect it to stall while attempting to interact with MDNS.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // There should now be a request from the publisher.
        let responder = match exec.run_until_stalled(&mut test_vals.publisher_stream.next()) {
            Poll::Ready(Some(Ok(
                fidl_mdns::ServiceInstancePublisherRequest::PublishServiceInstance {
                    service,
                    instance,
                    options,
                    publication_responder,
                    responder,
                },
            ))) => {
                assert_eq!(service, BORDER_AGENT_SERVICE_TYPE);
                assert_eq!(instance, TEST_SERVICE);
                assert_eq!(options, ServiceInstancePublicationOptions::default());
                responder.send(Ok(())).expect("Failed to send publish response");

                publication_responder
            }
            other => panic!("Unexpected variant: {:?}", other),
        };
        drop(responder);

        // The future should complete.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    #[fuchsia::test]
    fn test_publish_epskc_service_exits_when_publisher_drops_responder() {
        let mut exec = TestExecutor::new();
        let mut test_vals = test_setup();

        let fut = publish_epskc_service(String::from(TEST_SERVICE), TEST_PORT, test_vals.publisher);
        let mut fut = pin!(fut);

        // Progress the future and expect it to stall while attempting to interact with MDNS.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // There should now be a request from the publisher.
        let responder = match exec.run_until_stalled(&mut test_vals.publisher_stream.next()) {
            Poll::Ready(Some(Ok(
                fidl_mdns::ServiceInstancePublisherRequest::PublishServiceInstance {
                    service,
                    instance,
                    options,
                    publication_responder,
                    responder,
                },
            ))) => {
                assert_eq!(service, BORDER_AGENT_EPSKC_SERVICE_TYPE);
                assert_eq!(instance, TEST_SERVICE);
                assert_eq!(options, ServiceInstancePublicationOptions::default());
                responder.send(Ok(())).expect("Failed to send publish response");

                publication_responder
            }
            other => panic!("Unexpected variant: {:?}", other),
        };
        drop(responder);

        // The future should complete.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    #[fuchsia::test]
    fn test_manage_epskc_service_publisher_publishes_service() {
        let mut exec = TestExecutor::new();
        let mut test_vals = test_setup();

        let fut = manage_epskc_service_publisher(test_vals.receiver, test_vals.publisher.clone());
        let mut fut = pin!(fut);

        // Initially nothing happens.  The future should simply block waiting for requests.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Make a request to start.
        test_vals
            .sender
            .try_send(PublishServiceRequest::Start {
                port: TEST_PORT,
                service_instance: String::from(TEST_SERVICE),
            })
            .expect("failed to send start request");

        // Progress the future and observe that it has made a publication request.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = match exec.run_until_stalled(&mut test_vals.publisher_stream.next()) {
            Poll::Ready(Some(Ok(
                fidl_mdns::ServiceInstancePublisherRequest::PublishServiceInstance {
                    service,
                    instance,
                    options,
                    publication_responder,
                    responder,
                },
            ))) => {
                assert_eq!(service, BORDER_AGENT_EPSKC_SERVICE_TYPE);
                assert_eq!(instance, TEST_SERVICE);
                assert_eq!(options, ServiceInstancePublicationOptions::default());
                responder.send(Ok(())).expect("Failed to send publish response");

                publication_responder
            }
            other => panic!("Unexpected variant: {:?}", other),
        };

        // Progress the future again and observe that it is still running.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        let responder = responder.into_proxy();
        assert!(!responder.is_closed());
    }

    #[fuchsia::test]
    fn test_manage_epskc_service_publisher_stops_publishing() {
        let mut exec = TestExecutor::new();
        let mut test_vals = test_setup();

        let fut = manage_epskc_service_publisher(test_vals.receiver, test_vals.publisher.clone());
        let mut fut = pin!(fut);

        // Initially nothing happens.  The future should simply block waiting for requests.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Make a request to start.
        test_vals
            .sender
            .try_send(PublishServiceRequest::Start {
                port: TEST_PORT,
                service_instance: String::from(TEST_SERVICE),
            })
            .expect("failed to send start request");

        // Progress the future and observe that it has made a publication request.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = match exec.run_until_stalled(&mut test_vals.publisher_stream.next()) {
            Poll::Ready(Some(Ok(
                fidl_mdns::ServiceInstancePublisherRequest::PublishServiceInstance {
                    service,
                    instance,
                    options,
                    publication_responder,
                    responder,
                },
            ))) => {
                assert_eq!(service, BORDER_AGENT_EPSKC_SERVICE_TYPE);
                assert_eq!(instance, TEST_SERVICE);
                assert_eq!(options, ServiceInstancePublicationOptions::default());
                responder.send(Ok(())).expect("Failed to send publish response");

                publication_responder
            }
            other => panic!("Unexpected variant: {:?}", other),
        };

        // Progress the future again and observe that it is still running.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        let responder = responder.into_proxy();
        assert!(!responder.is_closed());

        // Now request that publishing stop.
        test_vals
            .sender
            .try_send(PublishServiceRequest::Stop)
            .expect("failed to make stop request");

        // Progress the future and observe that no new publication requests are made and the
        // existing publication channel has been dropped.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_vals.publisher_stream.next()),
            Poll::Pending
        );
        assert!(responder.is_closed())
    }

    #[fuchsia::test]
    fn test_manage_epskc_service_publisher_overlapping_start_requests() {
        let mut exec = TestExecutor::new();
        let mut test_vals = test_setup();

        let fut = manage_epskc_service_publisher(test_vals.receiver, test_vals.publisher.clone());
        let mut fut = pin!(fut);

        // Initially nothing happens.  The future should simply block waiting for requests.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Make a request to start.
        test_vals
            .sender
            .try_send(PublishServiceRequest::Start {
                port: TEST_PORT,
                service_instance: String::from(TEST_SERVICE),
            })
            .expect("failed to send start request");

        // Progress the future and observe that it has made a publication request.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        let responder = match exec.run_until_stalled(&mut test_vals.publisher_stream.next()) {
            Poll::Ready(Some(Ok(
                fidl_mdns::ServiceInstancePublisherRequest::PublishServiceInstance {
                    service,
                    instance,
                    options,
                    publication_responder,
                    responder,
                },
            ))) => {
                assert_eq!(service, BORDER_AGENT_EPSKC_SERVICE_TYPE);
                assert_eq!(instance, TEST_SERVICE);
                assert_eq!(options, ServiceInstancePublicationOptions::default());
                responder.send(Ok(())).expect("Failed to send publish response");

                publication_responder
            }
            other => panic!("Unexpected variant: {:?}", other),
        };

        // Progress the future again and observe that it is still running.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        let responder = responder.into_proxy();
        assert!(!responder.is_closed());

        // Make another start request.
        test_vals
            .sender
            .try_send(PublishServiceRequest::Start {
                port: TEST_PORT,
                service_instance: String::from(TEST_SERVICE),
            })
            .expect("failed to send overlapping start request");

        // Progress the future and observe that another publish request has been made and the
        // original responder channel is closed.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        let _second_responder = match exec.run_until_stalled(&mut test_vals.publisher_stream.next())
        {
            Poll::Ready(Some(Ok(
                fidl_mdns::ServiceInstancePublisherRequest::PublishServiceInstance {
                    service,
                    instance,
                    options,
                    publication_responder,
                    responder,
                },
            ))) => {
                assert_eq!(service, BORDER_AGENT_EPSKC_SERVICE_TYPE);
                assert_eq!(instance, TEST_SERVICE);
                assert_eq!(options, ServiceInstancePublicationOptions::default());
                responder.send(Ok(())).expect("Failed to send publish response");

                publication_responder
            }
            other => panic!("Unexpected variant: {:?}", other),
        };
        assert!(responder.is_closed());
    }
}
