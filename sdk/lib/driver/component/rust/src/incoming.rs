// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::marker::PhantomData;

use anyhow::{Context, Error, anyhow};
use cm_types::{IterablePath, RelativePath};
use fdf_sys::fdf_token_transfer;
use fidl::endpoints::{ClientEnd, DiscoverableProtocolMarker, ServiceMarker, ServiceProxy};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_io::Flags;
use fidl_next_bind::Service;
use fuchsia_component::client::{Connect, connect_to_service_instance_at_dir_svc};
use fuchsia_component::directory::{AsRefDirectory, Directory, open_directory_async};
use fuchsia_component::{DEFAULT_SERVICE_INSTANCE, SVC_DIR};
use log::error;
use namespace::{Entry, Namespace};
use zx::Status;

/// Implements access to the incoming namespace for a driver. It provides methods
/// for accessing incoming protocols and services by either their marker or proxy
/// types, and can be used as a [`Directory`] with the functions in
/// [`fuchsia_component::client`].
pub struct Incoming(Vec<Entry>);

impl Incoming {
    /// Connects to the protocol in the service instance's path in the incoming namespace. Logs and
    /// returns a [`Status::CONNECTION_REFUSED`] if the service instance couldn't be opened.
    pub fn connect_protocol<T: Connect>(&self) -> Result<T, Status> {
        T::connect_at_dir_svc(&self).map_err(|e| {
            error!(
                "Failed to connect to discoverable protocol `{}`: {e}",
                T::Protocol::PROTOCOL_NAME
            );
            Status::CONNECTION_REFUSED
        })
    }

    /// Connects to the protocol in the service instance's path in the given directory, with
    /// zx::Channel. Logs and returns a [`Status::CONNECTION_REFUSED`] if the service instance
    /// couldn't be opened.
    /// Connects to the protocol in the service instance's path in the incoming namespace, with
    /// `libasync_fidl::AsyncChannel`. Logs and returns a [`Status::CONNECTION_REFUSED`] if the
    /// service instance couldn't be opened.
    pub fn connect_protocol_libasync_next<P: fidl_next::Discoverable, D>(
        &self,
    ) -> Result<fidl_next::ClientEnd<P, libasync_fidl::AsyncChannel<D>>, Status>
    where
        D: Default,
    {
        let path = format!("/svc/{}", P::PROTOCOL_NAME);
        Self::connect_protocol_libasync_next_at(self, &path)
    }

    /// Connects to the protocol in the service instance's path in the given directory, with
    /// `libasync_fidl::AsyncChannel`. Logs and returns a [`Status::CONNECTION_REFUSED`] if the
    /// service instance couldn't be opened.
    pub fn connect_protocol_libasync_next_at<P: fidl_next::Discoverable, D>(
        dir: &impl AsRefDirectory,
        path: &str,
    ) -> Result<fidl_next::ClientEnd<P, libasync_fidl::AsyncChannel<D>>, Status>
    where
        D: Default,
    {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = fidl_next::ClientEnd::<P, zx::Channel>::from_untyped(client_end);
        dir.as_ref_directory().open(path, fio::Flags::PROTOCOL_SERVICE, server_end).map_err(
            |e| {
                error!("Failed to connect to discoverable protocol `{}`: {e}", P::PROTOCOL_NAME);
                Status::CONNECTION_REFUSED
            },
        )?;
        Ok(libasync_fidl::AsyncChannel::<D>::client_from_zx_channel::<P>(client_end))
    }

    /// Connects to the protocol in the service instance's path in the incoming namespace, with
    /// `zx::Channel`. Logs and returns a [`Status::CONNECTION_REFUSED`] if the service instance
    /// couldn't be opened.
    pub fn connect_protocol_next<P: fidl_next::Discoverable>(
        &self,
    ) -> Result<fidl_next::ClientEnd<P, zx::Channel>, Status> {
        let path = format!("/svc/{}", P::PROTOCOL_NAME);
        Self::connect_protocol_next_at(self, &path)
    }

    /// Connects to the protocol in the service instance's path in the given directory, with
    /// `zx::Channel`. Logs and returns a [`Status::CONNECTION_REFUSED`] if the service instance
    /// couldn't be opened.
    pub fn connect_protocol_next_at<P: fidl_next::Discoverable>(
        dir: &impl AsRefDirectory,
        path: &str,
    ) -> Result<fidl_next::ClientEnd<P, zx::Channel>, Status> {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = fidl_next::ClientEnd::<P, zx::Channel>::from_untyped(client_end);
        dir.as_ref_directory().open(path, fio::Flags::PROTOCOL_SERVICE, server_end).map_err(
            |e| {
                error!("Failed to connect to discoverable protocol `{}`: {e}", P::PROTOCOL_NAME);
                Status::CONNECTION_REFUSED
            },
        )?;
        Ok(client_end)
    }

    /// Connects to the protocol in the service instance's path in the given directory over driver
    /// transport, with [`fdf_fidl::DriverChannel`], using `dispatcher`. Logs and returns a
    /// [`Status::CONNECTION_REFUSED`] if the service instance couldn't be opened.
    pub fn connect_protocol_driver_transport<P: fidl_next::Discoverable, D>(
        &self,
        dispatcher: D,
    ) -> Result<fidl_next::ClientEnd<P, fdf_fidl::DriverChannel<D>>, zx::Status>
    where
        D: Clone,
    {
        let path = format!("/svc/{}", P::PROTOCOL_NAME);
        Self::connect_protocol_driver_transport_at(self, &path, dispatcher)
    }

    /// Connects to the protocol in the service instance's path in the given directory over driver
    /// transport, with [`fdf_fidl::DriverChannel`], using `dispatcher`. Logs and returns a
    /// [`Status::CONNECTION_REFUSED`] if the service instance couldn't be opened.
    pub fn connect_protocol_driver_transport_at<P: fidl_next::Discoverable, D>(
        dir: &impl AsRefDirectory,
        path: &str,
        dispatcher: D,
    ) -> Result<fidl_next::ClientEnd<P, fdf_fidl::DriverChannel<D>>, zx::Status>
    where
        D: Clone,
    {
        let (client_token, server_token) = zx::Channel::create();
        let (client_end, server_end) = fdf_fidl::DriverChannel::create_with_dispatcher(dispatcher);

        dir.as_ref_directory().open(path, fio::Flags::PROTOCOL_SERVICE, server_token).map_err(
            |e| {
                error!("Failed to connect to discoverable protocol `{}`: {e}", P::PROTOCOL_NAME);
                zx::Status::CONNECTION_REFUSED
            },
        )?;
        // SAFETY: client_token and server_end are valid by construction and
        // `fdf_token_transfer` consumes both handles and does not interact with rust memory.
        zx::Status::ok(unsafe {
            fdf_sys::fdf_token_transfer(
                client_token.into_raw(),
                server_end.into_driver_handle().into_raw().get(),
            )
        })
        .inspect_err(|e| {
            error!("Failed to connect to discoverable protocol `{}`: {e}", P::PROTOCOL_NAME);
        })?;

        Ok(fidl_next::ClientEnd::<P, _>::from_untyped(client_end))
    }

    /// Creates a connector to the given service's default instance by its marker type. This can be
    /// convenient when the compiler can't deduce the [`ServiceProxy`] type on its own.
    ///
    /// See [`ServiceConnector`] for more about what you can do with the connector.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let service = context.incoming.service_marker(fidl_fuchsia_hardware_i2c::ServiceMarker).connect()?;
    /// let device = service.connect_to_device()?;
    /// ```
    pub fn service_marker<M: ServiceMarker>(&self, _marker: M) -> ServiceConnector<'_, M::Proxy> {
        ServiceConnector { incoming: self, instance: DEFAULT_SERVICE_INSTANCE, _p: PhantomData }
    }

    /// Creates a connector to the given service's default instance by its proxy type. This can be
    /// convenient when the compiler can deduce the [`ServiceProxy`] type on its own.
    ///
    /// See [`ServiceConnector`] for more about what you can do with the connector.
    ///
    /// # Example
    ///
    /// ```ignore
    /// struct MyProxies {
    ///     i2c_service: fidl_fuchsia_hardware_i2c::ServiceProxy,
    /// }
    /// let proxies = MyProxies {
    ///     i2c_service: context.incoming.service().connect()?;
    /// };
    /// ```
    pub fn service<P>(&self) -> ServiceConnector<'_, P> {
        ServiceConnector { incoming: self, instance: DEFAULT_SERVICE_INSTANCE, _p: PhantomData }
    }
}

impl From<Vec<cm_types::NamespaceEntry>> for Incoming {
    fn from(other: Vec<cm_types::NamespaceEntry>) -> Self {
        Self(other.into_iter().map(Into::into).collect())
    }
}

/// A builder for connecting to an aggregated service instance in the driver's incoming namespace.
/// By default, it will connect to the default instance, named `default`. You can override this
/// by calling [`Self::instance`].
pub struct ServiceConnector<'incoming, ServiceProxy> {
    incoming: &'incoming Incoming,
    instance: &'incoming str,
    _p: PhantomData<ServiceProxy>,
}

impl<'a, S> ServiceConnector<'a, S> {
    /// Overrides the instance name to connect to when [`Self::connect`] is called.
    pub fn instance(self, instance: &'a str) -> Self {
        let Self { incoming, _p, .. } = self;
        Self { incoming, instance, _p }
    }
}

impl<'a, S: ServiceProxy> ServiceConnector<'a, S>
where
    S::Service: ServiceMarker,
{
    /// Connects to the service instance's path in the incoming namespace. Logs and returns
    /// a [`Status::CONNECTION_REFUSED`] if the service instance couldn't be opened.
    pub fn connect(self) -> Result<S, Status> {
        connect_to_service_instance_at_dir_svc::<S::Service>(self.incoming, self.instance).map_err(
            |e| {
                error!(
                    "Failed to connect to aggregated service connector `{}`, instance `{}`: {e}",
                    S::Service::SERVICE_NAME,
                    self.instance
                );
                Status::CONNECTION_REFUSED
            },
        )
    }
}

/// Used with [`ServiceHandlerAdapter`] as a connector to members of a service instance.
pub struct ServiceMemberConnector(fio::DirectoryProxy);

fn connect(
    dir: &fio::DirectoryProxy,
    member: &str,
    server_end: zx::Channel,
) -> Result<(), fidl::Error> {
    #[cfg(fuchsia_api_level_at_least = "27")]
    return dir.open(member, fio::Flags::PROTOCOL_SERVICE, &fio::Options::default(), server_end);
    #[cfg(not(fuchsia_api_level_at_least = "27"))]
    return dir.open3(member, fio::Flags::PROTOCOL_SERVICE, &fio::Options::default(), server_end);
}

impl fidl_next_protocol::ServiceConnector<zx::Channel> for ServiceMemberConnector {
    type Error = fidl::Error;
    fn connect_to_member(&self, member: &str, server_end: zx::Channel) -> Result<(), Self::Error> {
        connect(&self.0, member, server_end)
    }
}

impl fidl_next_protocol::ServiceConnector<fdf_fidl::DriverChannel> for ServiceMemberConnector {
    type Error = Status;
    fn connect_to_member(
        &self,
        member: &str,
        server_end: fdf_fidl::DriverChannel,
    ) -> Result<(), Self::Error> {
        let (client_token, server_token) = zx::Channel::create();

        // SAFETY: client_token and server_end are valid by construction and `fdf_token_transfer`
        // consumes both handles and does not interact with rust memory.
        Status::ok(unsafe {
            fdf_token_transfer(
                client_token.into_raw(),
                server_end.into_driver_handle().into_raw().get(),
            )
        })?;

        connect(&self.0, member, server_token).map_err(|err| {
            error!("Failed to connect to service member {member}: {err:?}");
            Status::CONNECTION_REFUSED
        })
    }
}

/// A type alias representing a service instance with members that can be connected to using the
/// [`fidl_next`] bindings.
pub type ServiceInstance<S> = fidl_next_bind::ServiceConnector<S, ServiceMemberConnector>;

impl<'a, S: Service<ServiceMemberConnector>> ServiceConnector<'a, ServiceInstance<S>> {
    /// Connects to the service instance's path in the incoming namespace with the new wire bindings.
    /// Logs and returns a [`Status::CONNECTION_REFUSED`] if the service instance couldn't be opened.
    pub fn connect_next(self) -> Result<ServiceInstance<S>, Status> {
        let service_path = format!("{SVC_DIR}/{}/{}", S::SERVICE_NAME, self.instance);
        let dir =
            open_directory_async(self.incoming, &service_path, fio::R_STAR_DIR).map_err(|e| {
                error!(
                    "Failed to connect to aggregated service connector `{}`, instance `{}`: {e}",
                    S::SERVICE_NAME,
                    self.instance
                );
                Status::CONNECTION_REFUSED
            })?;
        Ok(fidl_next_bind::ServiceConnector::from_untyped(ServiceMemberConnector(dir)))
    }
}

impl From<Namespace> for Incoming {
    fn from(value: Namespace) -> Self {
        Incoming(value.flatten())
    }
}

impl From<ClientEnd<fio::DirectoryMarker>> for Incoming {
    fn from(client: ClientEnd<fio::DirectoryMarker>) -> Self {
        Incoming(vec![Entry {
            path: cm_types::NamespacePath::new("/").unwrap(),
            directory: client,
        }])
    }
}

/// Returns the remainder of a prefix match of `prefix` against `self` in terms of path segments.
///
/// For example:
/// ```ignore
/// match_prefix("pkg/data", "pkg") == Some("/data")
/// match_prefix("pkg_data", "pkg") == None
/// ```
fn match_prefix(match_in: &impl IterablePath, prefix: &impl IterablePath) -> Option<RelativePath> {
    let mut my_segments = match_in.iter_segments();
    let mut prefix_segments = prefix.iter_segments();
    for prefix in prefix_segments.by_ref() {
        if prefix != my_segments.next()? {
            return None;
        }
    }
    if prefix_segments.next().is_some() {
        // did not match all prefix segments
        return None;
    }
    let segments = Vec::from_iter(my_segments);
    Some(RelativePath::from(segments))
}

impl Directory for Incoming {
    fn open(&self, path: &str, flags: Flags, server_end: zx::Channel) -> Result<(), Error> {
        let path = path.strip_prefix("/").unwrap_or(path);
        let path = RelativePath::new(path)?;

        for entry in &self.0 {
            if let Some(remain) = match_prefix(&path, &entry.path) {
                return entry.directory.open(&format!("{}", remain), flags, server_end);
            }
        }
        Err(Status::NOT_FOUND)
            .with_context(|| anyhow!("Path {path} not found in incoming namespace"))
    }
}

impl AsRefDirectory for Incoming {
    fn as_ref_directory(&self) -> &dyn Directory {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async::Task;
    use fuchsia_component::server::ServiceFs;
    use futures::stream::StreamExt;

    enum IncomingServices {
        Device(fidl_fuchsia_hardware_i2c::DeviceRequestStream),
        DefaultService(fidl_fuchsia_hardware_i2c::ServiceRequest),
        OtherService(fidl_fuchsia_hardware_i2c::ServiceRequest),
    }

    impl IncomingServices {
        async fn handle_device_stream(
            stream: fidl_fuchsia_hardware_i2c::DeviceRequestStream,
            name: &str,
        ) {
            stream
                .for_each(|msg| async move {
                    match msg.unwrap() {
                        fidl_fuchsia_hardware_i2c::DeviceRequest::GetName { responder } => {
                            responder.send(Ok(name)).unwrap();
                        }
                        _ => unimplemented!(),
                    }
                })
                .await
        }

        async fn handle(self) {
            use IncomingServices::*;
            match self {
                Device(stream) => Self::handle_device_stream(stream, "device").await,
                DefaultService(fidl_fuchsia_hardware_i2c::ServiceRequest::Device(stream)) => {
                    Self::handle_device_stream(stream, "default").await
                }
                OtherService(fidl_fuchsia_hardware_i2c::ServiceRequest::Device(stream)) => {
                    Self::handle_device_stream(stream, "other").await
                }
            }
        }
    }

    async fn make_incoming() -> Incoming {
        let (client, server) = fidl::endpoints::create_endpoints();
        let mut fs = ServiceFs::new();
        fs.dir("svc")
            .add_fidl_service(IncomingServices::Device)
            .add_fidl_service_instance("default", IncomingServices::DefaultService)
            .add_fidl_service_instance("other", IncomingServices::OtherService);
        fs.serve_connection(server).expect("error serving handle");

        Task::spawn(fs.for_each_concurrent(100, IncomingServices::handle)).detach_on_drop();
        Incoming::from(client)
    }

    #[fuchsia::test]
    async fn protocol_connect_present() -> anyhow::Result<()> {
        let incoming = make_incoming().await;
        // try a protocol that we did set up
        incoming
            .connect_protocol::<fidl_fuchsia_hardware_i2c::DeviceProxy>()?
            .get_name()
            .await?
            .unwrap();
        Ok(())
    }

    #[fuchsia::test]
    async fn protocol_connect_not_present() -> anyhow::Result<()> {
        let incoming = make_incoming().await;
        // try one we didn't
        incoming
            .connect_protocol::<fidl_fuchsia_hwinfo::DeviceProxy>()?
            .get_info()
            .await
            .unwrap_err();
        Ok(())
    }

    #[fuchsia::test]
    async fn service_connect_default_instance() -> anyhow::Result<()> {
        let incoming = make_incoming().await;
        // try the default service instance that we did set up
        assert_eq!(
            "default",
            &incoming
                .service_marker(fidl_fuchsia_hardware_i2c::ServiceMarker)
                .connect()?
                .connect_to_device()?
                .get_name()
                .await?
                .unwrap()
        );
        assert_eq!(
            "default",
            &incoming
                .service::<fidl_fuchsia_hardware_i2c::ServiceProxy>()
                .connect()?
                .connect_to_device()?
                .get_name()
                .await?
                .unwrap()
        );
        Ok(())
    }

    #[fuchsia::test]
    async fn service_connect_other_instance() -> anyhow::Result<()> {
        let incoming = make_incoming().await;
        // try the other service instance that we did set up
        assert_eq!(
            "other",
            &incoming
                .service_marker(fidl_fuchsia_hardware_i2c::ServiceMarker)
                .instance("other")
                .connect()?
                .connect_to_device()?
                .get_name()
                .await?
                .unwrap()
        );
        assert_eq!(
            "other",
            &incoming
                .service::<fidl_fuchsia_hardware_i2c::ServiceProxy>()
                .instance("other")
                .connect()?
                .connect_to_device()?
                .get_name()
                .await?
                .unwrap()
        );
        Ok(())
    }

    #[fuchsia::test]
    async fn service_connect_invalid_instance() -> anyhow::Result<()> {
        let incoming = make_incoming().await;
        // try the invalid service instance that we did not set up
        incoming
            .service_marker(fidl_fuchsia_hardware_i2c::ServiceMarker)
            .instance("invalid")
            .connect()?
            .connect_to_device()?
            .get_name()
            .await
            .unwrap_err();
        incoming
            .service::<fidl_fuchsia_hardware_i2c::ServiceProxy>()
            .instance("invalid")
            .connect()?
            .connect_to_device()?
            .get_name()
            .await
            .unwrap_err();
        Ok(())
    }
}
