// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::macros::{DriverRegistration, make_driver_registration};
use crate::testing::get_driver_from_token;
use crate::testing::harness::TestHarness;
use crate::testing::node::NodeHandle;
use crate::{Driver, Incoming};
use fdf::{AsAsyncDispatcherRef, AsyncDispatcher, DispatcherBuilder, OnDispatcher};
use fdf_env::Environment;
use fdf_fidl::DriverChannel;
use fidl_next::{Client as NextClient, ClientDispatcher, ClientEnd as NextClientEnd};
use fidl_next_fuchsia_driver_framework::{Driver as NextDriver, DriverStartArgs};
use fuchsia_async as fasync;
use futures::channel::oneshot;
use std::marker::PhantomData;
use std::sync::{Arc, mpsc};
use zx::Status;

/// This manages the driver under test's lifecycle.
pub struct DriverUnderTest<'a, D> {
    driver_outgoing: Incoming,
    driver: Option<fdf_env::Driver<u32>>,
    dispatcher: AsyncDispatcher,
    registration: DriverRegistration,
    token: usize,
    client: NextClient<NextDriver, DriverChannel>,
    client_exit_rx: Option<mpsc::Receiver<()>>,
    started: bool,
    harness: &'a mut TestHarness<D>,
    node_id: usize,
    _d: PhantomData<D>,
}

impl<D> Drop for DriverUnderTest<'_, D> {
    fn drop(&mut self) {
        if !self.started {
            self.client_exit_rx.take().expect("exit rx").recv().unwrap();
        }
        assert!(
            self.client_exit_rx.is_none(),
            "DriverUnderTest's stop_driver must be called before letting it go out of scope."
        );

        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let destroy_fn = self.registration.v1.destroy.unwrap();
        let driver_token = self.token;
        self.driver.take().expect("driver").shutdown(move |driver_ref| {
            // SAFETY: we created this through Box::into_raw below inside of new.
            let driver_value = unsafe { Box::from_raw(driver_ref.0 as *mut u32) };
            assert_eq!(*driver_value, 0x1337);

            // SAFETY: We ensures that the client_exit_rx has been called, which means that
            // the handle from initialize is dropped.
            unsafe {
                destroy_fn(driver_token as *mut _);
            }

            shutdown_tx.send(()).unwrap();
        });

        shutdown_rx.recv().unwrap();
    }
}

impl<'a, D: Driver> DriverUnderTest<'a, D> {
    pub(crate) async fn new(
        harness: &'a mut TestHarness<D>,
        fdf_env_environment: Arc<Environment>,
        driver_outgoing: Incoming,
        node_id: usize,
    ) -> Self {
        // Leak this to a raw, we will reconstitue a Box inside drop.
        let driver_value_ptr = Box::into_raw(Box::new(0x1337_u32));

        let driver = fdf_env_environment.new_driver(driver_value_ptr);
        let dispatcher_builder = DispatcherBuilder::new()
            .name("driver_under_test")
            .shutdown_observer(move |dispatcher| {
                // We verify that the dispatcher has no tasks left queued in it,
                // just because this is testing code.
                assert!(
                    !fdf_env_environment
                        .dispatcher_has_queued_tasks(dispatcher.as_dispatcher_ref())
                );
            });

        let registration = make_driver_registration::<D>();
        let dispatcher =
            AsyncDispatcher::new(&driver.new_dispatcher(dispatcher_builder).unwrap().release());
        let (server_chan, client_chan) = fdf::Channel::<[fidl_next::Chunk]>::create();
        let channel_handle = server_chan.into_driver_handle().into_raw().get();
        let (client_exit_tx, client_exit_rx) = mpsc::channel();
        let (token_tx, token_rx) = oneshot::channel();
        let initialize_fn = registration.v1.initialize.unwrap();
        dispatcher
            .post_task_sync(move |status| {
                assert_eq!(status, Ok(()));
                // SAFETY: We know it's safe to call initialize from the initial dispatcher and we
                // know channel_handle is non-zero.
                token_tx.send(unsafe { initialize_fn(channel_handle) }.addr()).unwrap();
            })
            .unwrap();
        let token = token_rx.await.unwrap();

        let client_end: NextClientEnd<NextDriver, DriverChannel> =
            NextClientEnd::from_untyped(DriverChannel::new(client_chan));
        let client_dispatcher = ClientDispatcher::new(client_end);
        let client = client_dispatcher.client();
        dispatcher.spawn(async move {
            // We have to manually run the client indefinitely until it returns a PEER_CLOSED.
            // At that point the driver has closed its server which signifies it has
            // completed its stop or has failed to start.
            client_dispatcher.run_client().await.unwrap_err();
            client_exit_tx.send(()).unwrap();
        });

        Self {
            driver_outgoing,
            driver: Some(driver),
            dispatcher,
            registration,
            token,
            client,
            client_exit_rx: Some(client_exit_rx),
            started: false,
            harness,
            node_id,
            _d: PhantomData,
        }
    }

    pub(crate) async fn start_driver(&mut self, start_args: DriverStartArgs) -> Result<(), Status> {
        self.client
            .start(start_args)
            .await
            .expect("start call success")
            .map_err(|e| e.err().unwrap_or(Status::INTERNAL))?;
        self.started = true;
        Ok(())
    }

    /// Allows the test to connect to capabilities that are provided by the driver through its
    /// outgoing namespace.
    pub fn driver_outgoing(&self) -> &Incoming {
        &self.driver_outgoing
    }

    /// Returns a reference to the driver instance.
    pub fn get_driver(&self) -> Option<&'_ D> {
        unsafe {
            // SAFETY: We know that the driver_token is valid and that the driver is of type T.
            get_driver_from_token(self.token)
        }
    }

    /// Gets the driver's initial dispatcher.
    pub fn dispatcher(&self) -> AsyncDispatcher {
        self.dispatcher.clone()
    }

    /// Gets the TestNode that the driver-under-test is bound to.
    pub fn node(&self) -> NodeHandle {
        NodeHandle::new(self.harness.node_manager(), self.node_id)
    }

    /// Get a reference to the harness that started the driver.
    pub fn harness(&self) -> &'_ TestHarness<D> {
        self.harness
    }

    /// Stop the driver.
    pub async fn stop_driver(mut self) {
        // We should only send a stop request if the driver started successfully.
        if self.started {
            // Sometimes the channel closes earlier than we get the stop result.
            let _stop_res = self.client.stop().await;
            let client_exit_rx = self.client_exit_rx.take().expect("exit rx");
            fasync::unblock(move || client_exit_rx.recv().unwrap()).await;
        }
    }
}
