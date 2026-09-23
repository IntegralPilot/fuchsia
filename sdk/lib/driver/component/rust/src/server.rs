// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ffi::c_void;
use core::ptr::NonNull;
use std::num::NonZero;
use std::ops::ControlFlow;
use std::sync::OnceLock;

use log::{debug, warn};
use zx::Status;

use fdf::{Channel, DispatcherBuilder, DriverDispatcherRef};
use fidl_fuchsia_driver_framework::DriverRequest;

use fdf::{AsAsyncDispatcherRef, AsyncDispatcher, DriverHandle, Message, fdf_handle_t};

use crate::{Driver, DriverContext, DriverError};
use fdf_sys::fdf_dispatcher_get_current_dispatcher;
use fidl_fuchsia_driver_framework::DriverStartArgs;
use fuchsia_async::LocalExecutorBuilder;

/// Implements the lifecycle management of a rust driver, including starting and stopping it
/// and setting up the rust async dispatcher and logging for the driver to use, and running a
/// message loop for the driver start and stop messages.
pub struct DriverServer<T> {
    server_handle: OnceLock<Channel<[u8]>>,
    root_dispatcher: DriverDispatcherRef<'static>,
    driver: OnceLock<T>,
}

impl<T: Driver> DriverServer<T> {
    /// Called by the driver host to start the driver.
    ///
    /// # Safety
    ///
    /// The caller must provide a valid non-zero driver transport channel handle for
    /// `server_handle`.
    pub unsafe extern "C" fn initialize(server_handle: fdf_handle_t) -> *mut c_void {
        // SAFETY: We verify that the pointer returned is non-null, ensuring that this was
        // called from within a driver context.
        let root_dispatcher = NonNull::new(unsafe { fdf_dispatcher_get_current_dispatcher() })
            .expect("Non-null current dispatcher");
        // SAFETY: We use NonZero::new to verify that we've been given a non-zero
        // driver handle, and expect that the caller (which is the driver runtime) has given us
        // a valid driver transport fidl channel.
        let server_handle = OnceLock::from(unsafe {
            Channel::from_driver_handle(DriverHandle::new_unchecked(
                NonZero::new(server_handle).expect("valid driver handle"),
            ))
        });

        // SAFETY: the root dispatcher is expected to live as long as this driver is loaded.
        let root_dispatcher = unsafe { DriverDispatcherRef::from_raw(root_dispatcher) };
        // We leak the box holding the server so that the driver runtime can take control over the
        // lifetime of the server object.
        let server_ptr = Box::into_raw(Box::new(Self {
            server_handle,
            root_dispatcher: root_dispatcher.clone(),
            driver: OnceLock::default(),
        }));

        // Reconstitute the pointer to the `DriverServer` as a mut reference to use it in the main
        // loop.
        // SAFETY: We are the exclusive owner of the object until we drop the server handle,
        // triggering the driver host to call `destroy`.
        let server = unsafe { &mut *server_ptr };

        // Build a new dispatcher that we can have spin on a fuchsia-async executor main loop
        // to act as a reactor for non-driver events. Use the always_on_dispatcher on it because
        // this thread is always running and we don't want to hold up the driver's dispatcher
        // suspension operation.
        let rust_async_dispatcher = DispatcherBuilder::new()
            .name("fuchsia-async")
            .allow_thread_blocking()
            .create_released()
            .expect("failure creating blocking dispatcher for rust async")
            .always_on_dispatcher();
        // Post the task to the dispatcher that will run the fuchsia-async loop, and have it run
        // the server's message loop waiting for start and stop messages from the driver host.
        let root_dispatcher_always_on = root_dispatcher.always_on_dispatcher();
        rust_async_dispatcher
            .post_task_sync(move |status| {
                // bail immediately if we were somehow cancelled before we started
                if status.is_err() {
                    return;
                }
                fdf_core::override_current_dispatcher(root_dispatcher.clone(), || {
                    // create and run a fuchsia-async executor, giving it the "root" dispatcher to
                    // actually execute driver tasks on, as this thread will be effectively blocked
                    // by the reactor loop.
                    let port = zx::Port::create_with_opts(zx::PortOptions::BIND_TO_INTERRUPT);
                    let mut executor = LocalExecutorBuilder::new().port(port).build();
                    executor.run_singlethreaded(async move {
                        server.message_loop(root_dispatcher_always_on).await;
                        // take the server handle so it can drop after the async block is done,
                        // which will signal to the driver host that the driver has finished
                        // shutdown, so that we are can guarantee that when `destroy` is called, we
                        // are not still using `server`.
                        server.server_handle.take()
                    });
                });
            })
            .expect("failure spawning main event loop for rust async dispatch");

        // Take the pointer of the server object to use as the identifier for the server to the
        // driver runtime. It uses this as an opaque identifier and expects no particular layout of
        // the object pointed to, and we use it to free the box at unload in `Self::destroy`.
        server_ptr.cast()
    }

    /// Called by the driver host after shutting down a driver and once the handle passed to
    /// [`Self::initialize`] is dropped.
    ///
    /// # Safety
    ///
    /// This must only be called after the handle provided to [`Self::initialize`] has been
    /// dropped, which indicates that the main event loop of the driver lifecycle has ended.
    pub unsafe extern "C" fn destroy(obj: *mut c_void) {
        let obj: *mut Self = obj.cast();
        // SAFETY: We built this object in `initialize` and gave ownership of its
        // lifetime to the driver framework, which is now giving it to us to free.
        unsafe { drop(Box::from_raw(obj)) }
    }

    /// Implements the main message loop for handling start and stop messages from rust
    /// driver host and passing them on to the implementation of [`Driver`] we contain.
    async fn message_loop(&mut self, dispatcher: DriverDispatcherRef<'_>) {
        loop {
            let server_handle_lock = self.server_handle.get_mut();
            let Some(server_handle) = server_handle_lock else {
                panic!("driver already shut down while message loop was running")
            };
            match server_handle.read_bytes(dispatcher.clone()).await {
                Ok(Some(message)) => {
                    if let ControlFlow::Break(_) = self.handle_message(message).await {
                        // driver shut down or failed to start, exit message loop
                        return;
                    }
                }
                Ok(None) => panic!("unexpected empty message on server channel"),
                Err(status @ Status::PEER_CLOSED) | Err(status @ Status::UNAVAILABLE) => {
                    warn!(
                        "Driver server channel closed before a stop message with status {status}, exiting main loop early but stop() will not be called."
                    );
                    return;
                }
                Err(e) => panic!("unexpected error on server channel {e}"),
            }
        }
    }

    /// Handles the start message by initializing logging and calling the [`Driver::start`] with
    /// a constructed [`DriverContext`].
    ///
    /// # Panics
    ///
    /// This method panics if the start message has already been received.
    async fn handle_start(&self, start_args: DriverStartArgs) -> Result<(), Status> {
        let context = DriverContext::new(AsyncDispatcher::new(&self.root_dispatcher), start_args)?;
        context.start_logging(T::NAME)?;

        log::debug!("driver starting");

        let driver = T::start(context).await.map_err(DriverError::log_to_status)?;
        self.driver.set(driver).map_err(|_| ()).expect("Driver received start message twice");
        Ok(())
    }

    async fn handle_stop(&mut self) {
        log::debug!("driver stopping");
        self.driver
            .take()
            .expect("received stop message more than once or without successfully starting")
            .stop()
            .await;
    }

    async fn handle_suspend(&self) -> Result<(), Status> {
        log::debug!("driver suspending");
        let driver = self.driver.get().expect("received suspend message without starting");
        driver.system_suspend().await.map_err(DriverError::log_to_status)
    }

    async fn handle_resume(&self, lease: Option<zx::EventPair>) -> Result<(), Status> {
        log::debug!("driver resuming");
        let driver = self.driver.get().expect("received resume message without starting");
        driver.system_resume(lease).await.map_err(DriverError::log_to_status)
    }

    /// Dispatches messages from the driver host to the appropriate implementation.
    ///
    /// # Panics
    ///
    /// This method panics if the messages are received out of order somehow (two start messages,
    /// stop before start, etc).
    async fn handle_message(&mut self, message: Message<[u8]>) -> ControlFlow<()> {
        let (_, request) = DriverRequest::read_from_message(message).unwrap();
        match request {
            DriverRequest::Start { start_args, responder } => {
                let res = self.handle_start(start_args).await.map_err(Status::into_raw);
                let Some(server_handle) = self.server_handle.get() else {
                    panic!("driver shutting down before it was finished starting")
                };
                responder.send_response(server_handle, res).unwrap();
                if res.is_ok() {
                    ControlFlow::Continue(())
                } else {
                    debug!("driver failed to start, exiting main loop");
                    ControlFlow::Break(())
                }
            }
            DriverRequest::Stop {} => {
                self.handle_stop().await;
                ControlFlow::Break(())
            }
            DriverRequest::Suspend { responder } => {
                let res = self.handle_suspend().await.map_err(Status::into_raw);
                let Some(server_handle) = self.server_handle.get() else {
                    panic!("driver shutting down before it was finished suspending")
                };
                responder.send_response(server_handle, res).unwrap();
                ControlFlow::Continue(())
            }
            DriverRequest::Resume { power_element_lease, responder } => {
                let res = self.handle_resume(power_element_lease).await.map_err(Status::into_raw);
                let Some(server_handle) = self.server_handle.get() else {
                    panic!("driver shutting down before it was finished resuming")
                };
                responder.send_response(server_handle, res).unwrap();
                ControlFlow::Continue(())
            }
            _ => panic!("Unknown message on server channel"),
        }
    }

    /// Returns a reference to the driver, if it has been started.
    /// This is meant to be used only for testing.
    pub(crate) fn testing_get_driver(&self) -> Option<&'_ T> {
        self.driver.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DriverError;

    use fdf::{CurrentDispatcher, OnDispatcher};
    use fdf_env::test::spawn_in_driver;
    use fidl_next_fuchsia_driver_framework::DriverClientHandler;
    use zx::Status;

    use fdf::Channel;
    use fidl_next::{ClientDispatcher, ClientEnd};

    #[derive(Default)]
    struct TestDriver {
        _not_empty: bool,
    }

    impl Driver for TestDriver {
        const NAME: &str = "test_driver";

        async fn start(context: DriverContext) -> Result<Self, DriverError> {
            let DriverContext { root_dispatcher, start_args, .. } = context;
            println!("created new test driver on dispatcher: {root_dispatcher:?}");
            println!("driver start message: {start_args:?}");
            Ok(Self::default())
        }
        async fn stop(&self) {
            println!("driver stop message");
        }
        async fn system_suspend(&self) -> Result<(), DriverError> {
            println!("driver suspend message");
            Ok(())
        }
        async fn system_resume(&self, _lease: Option<zx::EventPair>) -> Result<(), DriverError> {
            println!("driver resume message");
            Ok(())
        }
    }

    crate::driver_register!(TestDriver);

    #[derive(Debug)]
    struct DriverClient;
    impl DriverClientHandler for DriverClient {}

    #[test]
    fn register_driver() {
        assert_eq!(__fuchsia_driver_registration__.version, 1);
        let initialize_func = __fuchsia_driver_registration__.v1.initialize.expect("initializer");
        let destroy_func = __fuchsia_driver_registration__.v1.destroy.expect("destroy function");

        let (server_chan, client_chan) = Channel::<[fidl_next::Chunk]>::create();

        spawn_in_driver("driver registration", async move {
            let client_end: ClientEnd<fidl_next_fuchsia_driver_framework::Driver, _> =
                ClientEnd::from_untyped(fdf_fidl::DriverChannel::new(client_chan));
            let dispatcher = ClientDispatcher::new(client_end);
            let client = dispatcher.client();

            let client_task = CurrentDispatcher.spawn(async move {
                dispatcher.run(DriverClient).await.unwrap_err();
            });

            let channel_handle = server_chan.into_driver_handle().into_raw().get();
            let driver_server = unsafe { initialize_func(channel_handle) } as usize;
            assert_ne!(driver_server, 0);

            client
                .start(fidl_next_fuchsia_driver_framework::DriverStartArgs::default())
                .await
                .unwrap()
                .unwrap();

            client.suspend().await.unwrap().unwrap();
            client.resume(None::<zx::EventPair>).await.unwrap().unwrap();

            client.stop().await.unwrap();
            client_task.await.unwrap();

            unsafe {
                destroy_func(driver_server as *mut c_void);
            }
        })
    }

    struct TestDriverAnyhowSuccess;

    impl Driver for TestDriverAnyhowSuccess {
        const NAME: &str = "test_driver_anyhow_success";

        async fn start(_context: DriverContext) -> Result<Self, DriverError> {
            Ok(Self)
        }
        async fn stop(&self) {}
    }

    #[test]
    fn test_anyhow_success() {
        let registration = crate::macros::make_driver_registration::<TestDriverAnyhowSuccess>();
        let initialize_func = registration.v1.initialize.expect("initializer");
        let destroy_func = registration.v1.destroy.expect("destroy function");

        let (server_chan, client_chan) = Channel::<[fidl_next::Chunk]>::create();

        spawn_in_driver("driver anyhow success", async move {
            let client_end: ClientEnd<fidl_next_fuchsia_driver_framework::Driver, _> =
                ClientEnd::from_untyped(fdf_fidl::DriverChannel::new(client_chan));
            let dispatcher = ClientDispatcher::new(client_end);
            let client = dispatcher.client();

            let client_task = CurrentDispatcher.spawn(async move {
                dispatcher.run(DriverClient).await.unwrap_err();
            });

            let channel_handle = server_chan.into_driver_handle().into_raw().get();
            let driver_server = unsafe { initialize_func(channel_handle) } as usize;
            assert_ne!(driver_server, 0);

            client
                .start(fidl_next_fuchsia_driver_framework::DriverStartArgs::default())
                .await
                .unwrap()
                .unwrap();

            client.stop().await.unwrap();
            client_task.await.unwrap();

            unsafe {
                destroy_func(driver_server as *mut c_void);
            }
        })
    }

    struct TestDriverAnyhowFailure;

    impl Driver for TestDriverAnyhowFailure {
        const NAME: &str = "test_driver_anyhow_failure";

        async fn start(_context: DriverContext) -> Result<Self, DriverError> {
            Err(anyhow::anyhow!(Status::INVALID_ARGS).into())
        }
        async fn stop(&self) {}
    }

    #[test]
    fn test_anyhow_failure() {
        let registration = crate::macros::make_driver_registration::<TestDriverAnyhowFailure>();
        let initialize_func = registration.v1.initialize.expect("initializer");
        let destroy_func = registration.v1.destroy.expect("destroy function");

        let (server_chan, client_chan) = Channel::<[fidl_next::Chunk]>::create();

        spawn_in_driver("driver anyhow failure", async move {
            let client_end: ClientEnd<fidl_next_fuchsia_driver_framework::Driver, _> =
                ClientEnd::from_untyped(fdf_fidl::DriverChannel::new(client_chan));
            let dispatcher = ClientDispatcher::new(client_end);
            let client = dispatcher.client();

            let client_task = CurrentDispatcher.spawn(async move {
                dispatcher.run(DriverClient).await.unwrap_err();
            });

            let channel_handle = server_chan.into_driver_handle().into_raw().get();
            let driver_server = unsafe { initialize_func(channel_handle) } as usize;
            assert_ne!(driver_server, 0);

            let res = client
                .start(fidl_next_fuchsia_driver_framework::DriverStartArgs::default())
                .await
                .unwrap();

            assert_eq!(res.unwrap_err(), Err(Status::INVALID_ARGS));

            client_task.await.unwrap();

            unsafe {
                destroy_func(driver_server as *mut c_void);
            }
        })
    }

    struct TestDriverAnyhowFailureDefault;

    impl Driver for TestDriverAnyhowFailureDefault {
        const NAME: &str = "test_driver_anyhow_failure_default";

        async fn start(_context: DriverContext) -> Result<Self, DriverError> {
            Err(anyhow::anyhow!("some generic error").into())
        }
        async fn stop(&self) {}
    }

    #[test]
    fn test_anyhow_failure_default() {
        let registration =
            crate::macros::make_driver_registration::<TestDriverAnyhowFailureDefault>();
        let initialize_func = registration.v1.initialize.expect("initializer");
        let destroy_func = registration.v1.destroy.expect("destroy function");

        let (server_chan, client_chan) = Channel::<[fidl_next::Chunk]>::create();

        spawn_in_driver("driver anyhow failure default", async move {
            let client_end: ClientEnd<fidl_next_fuchsia_driver_framework::Driver, _> =
                ClientEnd::from_untyped(fdf_fidl::DriverChannel::new(client_chan));
            let dispatcher = ClientDispatcher::new(client_end);
            let client = dispatcher.client();

            let client_task = CurrentDispatcher.spawn(async move {
                dispatcher.run(DriverClient).await.unwrap_err();
            });

            let channel_handle = server_chan.into_driver_handle().into_raw().get();
            let driver_server = unsafe { initialize_func(channel_handle) } as usize;
            assert_ne!(driver_server, 0);

            let res = client
                .start(fidl_next_fuchsia_driver_framework::DriverStartArgs::default())
                .await
                .unwrap();

            assert_eq!(res.unwrap_err(), Err(Status::INTERNAL));

            client_task.await.unwrap();

            unsafe {
                destroy_func(driver_server as *mut c_void);
            }
        })
    }
}
