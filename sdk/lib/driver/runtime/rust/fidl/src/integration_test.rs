// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_next::{ClientEnd, Responder, ServerEnd};
use fidl_next_fuchsia_examples_gizmo::device::{GetEvent, GetHardwareId};
use fidl_next_fuchsia_examples_gizmo::{Device, DeviceServerHandler};
use fuchsia_async::OnSignals;
use zx::{Event, Signals};

use fdf_env::test::spawn_in_driver;
use fdf_fidl::DriverChannel;

struct DeviceServer;
impl DeviceServerHandler for DeviceServer {
    async fn get_hardware_id(&mut self, responder: Responder<GetHardwareId>) {
        responder.respond(4004u32).await.unwrap();
    }

    async fn get_event(&mut self, responder: Responder<GetEvent>) {
        let event = Event::create();
        event.signal(Signals::empty(), Signals::USER_0).unwrap();
        responder.respond(event).await.unwrap();
    }
}

#[fuchsia::test]
async fn driver_fidl_server() {
    let res = spawn_in_driver("driver fidl server", async {
        let (server_chan, client_chan) = DriverChannel::create();
        let client_end: ClientEnd<Device, _> = ClientEnd::from_untyped(client_chan);
        let server_end: ServerEnd<Device, _> = ServerEnd::from_untyped(server_chan);
        server_end.spawn(DeviceServer);
        let client = client_end.spawn();

        let res = client.get_hardware_id().await.unwrap();
        let hardware_id = res.unwrap();
        assert_eq!(hardware_id.response, 4004);

        client.get_event().await.unwrap()
    });

    // wait for the event on a fuchsia_async executor
    let signalled = OnSignals::new(res.event, Signals::USER_0).await.unwrap();
    assert_eq!(Signals::USER_0, signalled);
}
