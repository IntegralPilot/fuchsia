// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fifo::{FIFO_SIZE, Fifo};
use fidl::endpoints::{DiscoverableProtocolMarker, RequestStream, Responder};
use fidl_fuchsia_device::DeviceSignal;
use fidl_fuchsia_hardware_pty::{self as fpty, DeviceRequest, DeviceRequestStream, WindowSize};
use fuchsia_async as fasync;
use futures::TryStreamExt;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::rc::Rc;
use zx::Peered;

const FEATURE_RAW: u32 = 1;

pub struct Pty {
    server: ServerState,
    clients: HashMap<u32, ClientState>,
    active_id: Option<u32>,
    control_id: Option<u32>,
    events: u32,
    window_size: WindowSize,
    server_connection_count: usize,
}

struct ServerState {
    event: zx::EventPair,
    fifo: Fifo,
    remote_event: zx::EventPair,
}

struct ClientState {
    event: zx::EventPair,
    remote_event: zx::EventPair,
    fifo: Fifo,
    flags: u32,
    connection_count: usize,
}

impl Pty {
    pub fn new() -> Self {
        let (local, remote) = zx::EventPair::create();
        local
            .signal_peer(
                zx::Signals::NONE,
                zx::Signals::from_bits_truncate(
                    DeviceSignal::READABLE.bits() | DeviceSignal::HANGUP.bits(),
                ),
            )
            .expect("event should be signalable");

        let fifo_event =
            local.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("handle should be duplicatable");
        Self {
            server: ServerState { event: local, remote_event: remote, fifo: Fifo::new(fifo_event) },
            clients: HashMap::new(),
            active_id: None,
            control_id: None,
            events: 0,
            window_size: WindowSize { width: 0, height: 0 },
            server_connection_count: 0,
        }
    }

    pub fn create_client(&mut self, id: u32) -> Result<(), zx::Status> {
        let empty = self.clients.is_empty();
        let entry = self.clients.entry(id);
        if matches!(entry, Entry::Occupied(_)) {
            return Err(zx::Status::INVALID_ARGS);
        }

        let (local, remote) = zx::EventPair::create();

        if empty {
            self.server
                .event
                .signal_peer(
                    zx::Signals::from_bits_truncate(
                        DeviceSignal::READABLE.bits() | DeviceSignal::HANGUP.bits(),
                    ),
                    zx::Signals::NONE,
                )
                .map_err(|_| zx::Status::INTERNAL)?;
        }

        let fifo_event = local
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .expect("event pair should be duplicatable");
        let mut client = ClientState {
            event: local,
            remote_event: remote,
            fifo: Fifo::new(fifo_event),
            flags: 0,
            connection_count: 0,
        };

        if id == 0 {
            self.control_id = Some(0);
            if self.events != 0 {
                client.assert_signal(DeviceSignal::OOB)?;
            }
        }

        let is_active = self.active_id.is_none();
        if is_active {
            self.active_id = Some(id);
            let mut to_clear = DeviceSignal::HANGUP.bits();
            let mut to_set = 0;
            if client.fifo.is_full() {
                to_clear |= DeviceSignal::WRITABLE.bits();
            } else {
                to_set |= DeviceSignal::WRITABLE.bits();
            }
            self.server
                .event
                .signal_peer(
                    zx::Signals::from_bits_truncate(to_clear),
                    zx::Signals::from_bits_truncate(to_set),
                )
                .map_err(|_| zx::Status::INTERNAL)?;
        }

        client.adjust_signals(is_active)?;

        let _ = entry.insert_entry(client);
        Ok(())
    }

    pub fn make_active(&mut self, id: u32) -> Result<(), zx::Status> {
        let old_id = self.active_id;
        {
            let Some(client) = self.clients.get_mut(&id) else {
                return Err(zx::Status::NOT_FOUND);
            };

            if self.active_id == Some(id) {
                return Ok(());
            }

            self.active_id = Some(id);
            client.assert_signal(DeviceSignal::WRITABLE)?;

            let mut to_clear = DeviceSignal::HANGUP.bits();
            let mut to_set = 0;
            if client.fifo.is_full() {
                to_clear |= DeviceSignal::WRITABLE.bits();
            } else {
                to_set |= DeviceSignal::WRITABLE.bits();
            }

            self.server
                .event
                .signal_peer(
                    zx::Signals::from_bits_truncate(to_clear),
                    zx::Signals::from_bits_truncate(to_set),
                )
                .map_err(|_| zx::Status::INTERNAL)?;
        }

        if let Some(old_id) = old_id
            && let Some(old_client) = self.clients.get_mut(&old_id)
        {
            old_client.deassert_signal(DeviceSignal::WRITABLE)?;
        }

        Ok(())
    }

    pub fn remove_client(&mut self, id: u32) {
        if self.clients.remove(&id).is_some() {
            self.control_id.take_if(|cid| *cid == id);
            if self.active_id == Some(id) {
                if let Some(control_id) = self.control_id
                    && let Some(control) = self.clients.get_mut(&control_id)
                {
                    // This may fail if the other side has closed their end and that's okay.
                    let _ = control.event.signal_peer(
                        zx::Signals::NONE,
                        zx::Signals::from_bits_truncate(
                            DeviceSignal::OOB.bits() | DeviceSignal::HANGUP.bits(),
                        ),
                    );
                }
                self.active_id = None;
            }
        }

        if self.clients.is_empty() {
            let _ = self.server.event.signal_peer(
                zx::Signals::from_bits_truncate(DeviceSignal::WRITABLE.bits()),
                zx::Signals::from_bits_truncate(
                    DeviceSignal::READABLE.bits() | DeviceSignal::HANGUP.bits(),
                ),
            );
        }
    }

    pub fn server_read(&mut self, count: usize) -> Result<Vec<u8>, zx::Status> {
        let data = match self.server.fifo.read(count) {
            Ok(d) => d,
            Err(zx::Status::SHOULD_WAIT) => {
                // If there are no clients we return EOF.
                if self.clients.is_empty() {
                    return Ok(Vec::new());
                } else {
                    return Err(zx::Status::SHOULD_WAIT);
                }
            }
            Err(e) => return Err(e),
        };

        if !data.is_empty() {
            self.assert_active_writable()?;
        }
        Ok(data)
    }

    pub fn server_write(&mut self, data: &[u8]) -> Result<usize, zx::Status> {
        let active_id = self.active_id.ok_or(zx::Status::PEER_CLOSED)?;

        if data.is_empty() {
            return Ok(0);
        }

        let client = self.clients.get_mut(&active_id).expect("active_id should be a valid client");

        if client.fifo.is_full() {
            return Err(zx::Status::SHOULD_WAIT);
        }

        let was_empty = client.fifo.is_empty();

        let mut evt = 0;
        let written = if (client.flags & FEATURE_RAW) != 0 {
            client.fifo.write(data, false)?
        } else {
            let mut len = std::cmp::min(data.len(), FIFO_SIZE);
            for (i, &b) in data[0..len].iter().enumerate() {
                if b == 0x03 {
                    evt = fpty::EVENT_INTERRUPT;
                    len = i;
                    break;
                }
            }

            let mut bytes_written = client.fifo.write(&data[0..len], false)?;
            if bytes_written == len && evt != 0 {
                bytes_written += 1;
            }
            bytes_written
        };

        if was_empty && !client.fifo.is_empty() {
            client.assert_signal(DeviceSignal::READABLE)?;
        }
        if client.fifo.is_full() {
            self.server
                .event
                .signal_peer(
                    zx::Signals::from_bits_truncate(DeviceSignal::WRITABLE.bits()),
                    zx::Signals::NONE,
                )
                .map_err(|_| zx::Status::INTERNAL)?;
        }

        if evt != 0 {
            self.events |= evt;
            if let Some(control_id) = self.control_id
                && let Some(control) = self.clients.get_mut(&control_id)
            {
                // The other side could have closed it's end which is okay.
                let _ = control.assert_signal(DeviceSignal::OOB);
            }
        }

        Ok(written)
    }

    pub fn client_read(&mut self, id: u32, count: usize) -> Result<Vec<u8>, zx::Status> {
        let client = self.clients.get_mut(&id).ok_or(zx::Status::PEER_CLOSED)?;
        let was_full = client.fifo.is_full();
        let data = match client.fifo.read(count) {
            Ok(d) => d,
            Err(zx::Status::SHOULD_WAIT) => {
                if self.server_connection_count == 0 {
                    return Err(zx::Status::PEER_CLOSED);
                } else {
                    return Err(zx::Status::SHOULD_WAIT);
                }
            }
            Err(e) => return Err(e),
        };

        if client.fifo.is_empty() {
            client.deassert_signal(DeviceSignal::READABLE)?;
        }

        if was_full && !data.is_empty() {
            self.server
                .event
                .signal_peer(
                    zx::Signals::NONE,
                    zx::Signals::from_bits_truncate(DeviceSignal::WRITABLE.bits()),
                )
                .map_err(|_| zx::Status::INTERNAL)?;
        }
        Ok(data)
    }

    pub fn client_write(&mut self, id: u32, data: &[u8]) -> Result<usize, zx::Status> {
        if self.server_connection_count == 0 {
            return Err(zx::Status::PEER_CLOSED);
        }
        if self.active_id != Some(id) {
            if !self.clients.contains_key(&id) {
                return Err(zx::Status::PEER_CLOSED);
            }
            return Err(zx::Status::SHOULD_WAIT);
        }
        if data.is_empty() {
            return Ok(0);
        }

        let raw_mode = {
            let client = self.clients.get(&id).ok_or(zx::Status::PEER_CLOSED)?;
            (client.flags & FEATURE_RAW) != 0
        };

        if raw_mode { self.client_write_raw(data) } else { self.client_write_cooked(data) }
    }

    fn client_write_raw(&mut self, data: &[u8]) -> Result<usize, zx::Status> {
        let written = self.write_to_server(data, false)?;
        if written > 0 { Ok(written) } else { Err(zx::Status::SHOULD_WAIT) }
    }

    fn client_write_cooked(&mut self, data: &[u8]) -> Result<usize, zx::Status> {
        let mut total_written = 0;

        for part in data.split_inclusive(|&b| b == b'\n') {
            let (body, has_newline) = match part.strip_suffix(b"\n") {
                Some(prefix) => (prefix, true),
                None => (part, false),
            };

            if !body.is_empty() {
                let written = self.write_to_server(body, false)?;
                total_written += written;
                if written < body.len() {
                    break;
                }
            }

            if has_newline {
                let written = self.write_to_server(b"\r\n", true)?;
                if written < 2 {
                    break;
                }
                total_written += 1;
            }
        }

        if total_written > 0 { Ok(total_written) } else { Err(zx::Status::SHOULD_WAIT) }
    }

    fn write_to_server(&mut self, data: &[u8], atomic: bool) -> Result<usize, zx::Status> {
        let was_empty = self.server.fifo.is_empty();
        let written = match self.server.fifo.write(data, atomic) {
            Ok(n) => n,
            Err(zx::Status::SHOULD_WAIT) => 0,
            Err(e) => return Err(e),
        };

        if was_empty && written > 0 {
            self.server
                .event
                .signal_peer(
                    zx::Signals::NONE,
                    zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
                )
                .map_err(|_| zx::Status::INTERNAL)?;
        }

        if written < data.len() || self.server.fifo.is_full() {
            self.deassert_active_writable()?;
        }

        Ok(written)
    }

    fn assert_active_writable(&mut self) -> Result<(), zx::Status> {
        if let Some(id) = self.active_id
            && let Some(client) = self.clients.get_mut(&id)
        {
            client.assert_signal(DeviceSignal::WRITABLE)?;
        }
        Ok(())
    }

    fn deassert_active_writable(&mut self) -> Result<(), zx::Status> {
        if let Some(id) = self.active_id
            && let Some(client) = self.clients.get_mut(&id)
        {
            client.deassert_signal(DeviceSignal::WRITABLE)?;
        }
        Ok(())
    }

    pub fn handle_server_close(&mut self) {
        for client in self.clients.values_mut() {
            let _ = client.event.signal_peer(
                zx::Signals::from_bits_truncate(DeviceSignal::WRITABLE.bits()),
                zx::Signals::from_bits_truncate(DeviceSignal::HANGUP.bits()),
            );
        }
        self.active_id = None;
    }

    pub fn clr_set_feature(&mut self, id: u32, clr: u32, set: u32) -> Result<u32, zx::Status> {
        let client = self.clients.get_mut(&id).ok_or(zx::Status::PEER_CLOSED)?;
        if (clr & !FEATURE_RAW) != 0 || (set & !FEATURE_RAW) != 0 {
            return Err(zx::Status::NOT_SUPPORTED);
        }
        client.flags = (client.flags & !clr) | set;
        Ok(client.flags)
    }
}

impl ClientState {
    fn adjust_signals(&mut self, is_active: bool) -> Result<(), zx::Status> {
        let mut to_clear = 0;
        let mut to_set = 0;

        if is_active {
            to_set |= DeviceSignal::WRITABLE.bits();
        } else {
            to_clear |= DeviceSignal::WRITABLE.bits();
        }

        if self.fifo.is_empty() {
            to_clear |= DeviceSignal::READABLE.bits();
        } else {
            to_set |= DeviceSignal::READABLE.bits();
        }

        self.event
            .signal_peer(
                zx::Signals::from_bits_truncate(to_clear),
                zx::Signals::from_bits_truncate(to_set),
            )
            .map_err(|_| zx::Status::INTERNAL)?;
        Ok(())
    }

    fn assert_signal(&mut self, signal: DeviceSignal) -> Result<(), zx::Status> {
        self.event
            .signal_peer(zx::Signals::NONE, zx::Signals::from_bits_truncate(signal.bits()))
            .map_err(|_| zx::Status::INTERNAL)?;
        Ok(())
    }

    fn deassert_signal(&mut self, signal: DeviceSignal) -> Result<(), zx::Status> {
        self.event
            .signal_peer(zx::Signals::from_bits_truncate(signal.bits()), zx::Signals::NONE)
            .map_err(|_| zx::Status::INTERNAL)?;
        Ok(())
    }
}

fn ignore_peer_closed(err: fidl::Error) -> Result<(), fidl::Error> {
    if err.is_closed() { Ok(()) } else { Err(err) }
}

pub async fn run_server(pty: Rc<RefCell<Pty>>, stream: DeviceRequestStream) {
    pty.borrow_mut().server_connection_count += 1;

    if let Err(e) = run_server_internal(pty.clone(), stream).await {
        eprintln!("Server exited with error: {e}");
    }

    let mut pty = pty.borrow_mut();
    assert!(pty.server_connection_count > 0);
    pty.server_connection_count -= 1;
    if pty.server_connection_count == 0 {
        pty.handle_server_close();
    }
}

async fn run_server_internal(
    pty: Rc<RefCell<Pty>>,
    mut stream: DeviceRequestStream,
) -> Result<(), fidl::Error> {
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            DeviceRequest::OpenClient { id, client, responder } => {
                let mut pty_guard = pty.borrow_mut();
                match pty_guard.create_client(id) {
                    Ok(()) => {
                        responder.send(zx::sys::ZX_OK).or_else(ignore_peer_closed)?;
                        let client_stream = client.into_stream();
                        let pty_clone = pty.clone();
                        fasync::Task::local(async move {
                            run_client(pty_clone, id, client_stream.cast_stream()).await;
                        })
                        .detach();
                    }
                    Err(s) => {
                        responder.send(s.into_raw()).or_else(ignore_peer_closed)?;
                    }
                }
            }
            DeviceRequest::Read { count, responder } => {
                let mut pty = pty.borrow_mut();
                match pty.server_read(count as usize) {
                    Ok(data) => {
                        responder.send(Ok(&data)).or_else(ignore_peer_closed)?;
                    }
                    Err(s) => {
                        responder.send(Err(s.into_raw())).or_else(ignore_peer_closed)?;
                    }
                }
            }
            DeviceRequest::Write { data, responder } => {
                let mut pty = pty.borrow_mut();
                match pty.server_write(&data) {
                    Ok(written) => {
                        responder.send(Ok(written as u64)).or_else(ignore_peer_closed)?;
                    }
                    Err(s) => {
                        responder.send(Err(s.into_raw())).or_else(ignore_peer_closed)?;
                    }
                }
            }
            DeviceRequest::Describe { responder } => {
                let pty = pty.borrow();
                match pty.server.remote_event.duplicate_handle(zx::Rights::BASIC) {
                    Ok(event) => {
                        let _ = responder.send(fpty::DeviceDescribeResponse {
                            event: Some(event),
                            ..Default::default()
                        });
                    }
                    Err(s) => {
                        responder.control_handle().shutdown_with_epitaph(s);
                    }
                }
            }
            DeviceRequest::SetWindowSize { size, responder } => {
                let mut pty = pty.borrow_mut();
                pty.window_size = size;
                pty.events |= fpty::EVENT_WINDOW_SIZE;
                if let Some(control_id) = pty.control_id
                    && let Some(control) = pty.clients.get_mut(&control_id)
                {
                    let _ = control.assert_signal(DeviceSignal::OOB);
                }
                responder.send(zx::sys::ZX_OK).or_else(ignore_peer_closed)?;
            }
            DeviceRequest::Clone { request, .. } => {
                let client_stream = request.into_stream();
                let pty_clone = pty.clone();
                fasync::Task::local(async move {
                    run_server(pty_clone, client_stream.cast_stream()).await;
                })
                .detach();
            }
            DeviceRequest::Close { responder } => {
                responder.send(Ok(())).or_else(ignore_peer_closed)?;
                break;
            }
            DeviceRequest::Query { responder } => {
                responder
                    .send(fpty::DeviceMarker::PROTOCOL_NAME.as_bytes())
                    .or_else(ignore_peer_closed)?;
            }
            DeviceRequest::ClrSetFeature { responder, .. } => {
                responder
                    .send(zx::Status::NOT_SUPPORTED.into_raw(), 0)
                    .or_else(ignore_peer_closed)?;
            }
            DeviceRequest::GetWindowSize { responder, .. } => {
                let _ = responder.send(
                    zx::Status::NOT_SUPPORTED.into_raw(),
                    &WindowSize { width: 0, height: 0 },
                );
            }
            DeviceRequest::MakeActive { responder, .. } => {
                responder.send(zx::Status::NOT_SUPPORTED.into_raw()).or_else(ignore_peer_closed)?;
            }
            DeviceRequest::ReadEvents { responder, .. } => {
                responder
                    .send(zx::Status::NOT_SUPPORTED.into_raw(), 0)
                    .or_else(ignore_peer_closed)?;
            }
        }
    }
    Ok(())
}

async fn run_client(pty: Rc<RefCell<Pty>>, id: u32, stream: DeviceRequestStream) {
    if let Some(client) = pty.borrow_mut().clients.get_mut(&id) {
        client.connection_count += 1;
    } else {
        return;
    }

    if let Err(e) = run_client_internal(pty.clone(), id, stream).await {
        eprintln!("client exited with error: {e}");
    }

    let mut pty = pty.borrow_mut();
    if let Some(client) = pty.clients.get_mut(&id) {
        assert!(client.connection_count > 0);
        client.connection_count -= 1;
        if client.connection_count == 0 {
            pty.remove_client(id);
        }
    }
}

async fn run_client_internal(
    pty: Rc<RefCell<Pty>>,
    id: u32,
    mut stream: DeviceRequestStream,
) -> Result<(), fidl::Error> {
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            DeviceRequest::Read { count, responder } => {
                match pty.borrow_mut().client_read(id, count as usize) {
                    Ok(data) => {
                        responder.send(Ok(&data)).or_else(ignore_peer_closed)?;
                    }
                    Err(s) => {
                        responder.send(Err(s.into_raw())).or_else(ignore_peer_closed)?;
                    }
                }
            }
            DeviceRequest::Write { data, responder } => {
                match pty.borrow_mut().client_write(id, &data) {
                    Ok(written) => {
                        responder.send(Ok(written as u64)).or_else(ignore_peer_closed)?;
                    }
                    Err(s) => {
                        responder.send(Err(s.into_raw())).or_else(ignore_peer_closed)?;
                    }
                }
            }
            DeviceRequest::Describe { responder } => {
                if let Some(client) = pty.borrow_mut().clients.get(&id) {
                    match client.remote_event.duplicate_handle(zx::Rights::BASIC) {
                        Ok(event) => {
                            responder
                                .send(fpty::DeviceDescribeResponse {
                                    event: Some(event),
                                    ..Default::default()
                                })
                                .or_else(ignore_peer_closed)?;
                        }
                        Err(s) => {
                            responder.control_handle().shutdown_with_epitaph(s);
                        }
                    }
                } else {
                    responder.control_handle().shutdown_with_epitaph(zx::Status::PEER_CLOSED);
                }
            }
            DeviceRequest::OpenClient { id: new_id, client, responder } => {
                let mut pty_guard = pty.borrow_mut();
                if pty_guard.control_id != Some(id) {
                    responder
                        .send(zx::Status::ACCESS_DENIED.into_raw())
                        .or_else(ignore_peer_closed)?;
                } else if new_id == 0 {
                    responder
                        .send(zx::Status::INVALID_ARGS.into_raw())
                        .or_else(ignore_peer_closed)?;
                } else {
                    match pty_guard.create_client(new_id) {
                        Ok(()) => {
                            responder.send(zx::sys::ZX_OK).or_else(ignore_peer_closed)?;
                            let client_stream = client.into_stream();
                            let pty_clone = pty.clone();
                            fasync::Task::local(async move {
                                run_client(pty_clone, new_id, client_stream.cast_stream()).await;
                            })
                            .detach();
                        }
                        Err(s) => {
                            responder.send(s.into_raw()).or_else(ignore_peer_closed)?;
                        }
                    }
                }
            }
            DeviceRequest::ClrSetFeature { clr, set, responder } => {
                match pty.borrow_mut().clr_set_feature(id, clr, set) {
                    Ok(features) => {
                        responder.send(zx::sys::ZX_OK, features).or_else(ignore_peer_closed)?;
                    }
                    Err(s) => {
                        responder.send(s.into_raw(), 0).or_else(ignore_peer_closed)?;
                    }
                }
            }
            DeviceRequest::GetWindowSize { responder } => {
                responder
                    .send(zx::sys::ZX_OK, &pty.borrow().window_size)
                    .or_else(ignore_peer_closed)?;
            }
            DeviceRequest::MakeActive { client_pty_id, responder } => {
                let mut pty = pty.borrow_mut();
                if pty.control_id != Some(id) {
                    responder
                        .send(zx::Status::ACCESS_DENIED.into_raw())
                        .or_else(ignore_peer_closed)?;
                } else {
                    match pty.make_active(client_pty_id) {
                        Ok(()) => {
                            responder.send(zx::sys::ZX_OK).or_else(ignore_peer_closed)?;
                        }
                        Err(s) => {
                            responder.send(s.into_raw()).or_else(ignore_peer_closed)?;
                        }
                    }
                }
            }
            DeviceRequest::ReadEvents { responder } => {
                let mut pty = pty.borrow_mut();
                if pty.control_id != Some(id) {
                    responder
                        .send(zx::Status::ACCESS_DENIED.into_raw(), 0)
                        .or_else(ignore_peer_closed)?;
                } else {
                    let events = pty.events;
                    pty.events = 0;

                    if let Some(control_id) = pty.control_id
                        && let Some(control) = pty.clients.get_mut(&control_id)
                    {
                        let _ = control.deassert_signal(DeviceSignal::OOB);
                    }

                    let mut ret_events = events;
                    if pty.active_id.is_none() {
                        ret_events |= fpty::EVENT_HANGUP;
                    }

                    responder.send(zx::sys::ZX_OK, ret_events).or_else(ignore_peer_closed)?;
                }
            }
            DeviceRequest::SetWindowSize { size, responder } => {
                let mut pty = pty.borrow_mut();
                pty.window_size = size;
                pty.events |= fpty::EVENT_WINDOW_SIZE;
                if let Some(control_id) = pty.control_id
                    && let Some(control) = pty.clients.get_mut(&control_id)
                {
                    let _ = control.assert_signal(DeviceSignal::OOB);
                }
                responder.send(zx::sys::ZX_OK).or_else(ignore_peer_closed)?;
            }
            DeviceRequest::Clone { request, .. } => {
                let client_stream = request.into_stream();
                let pty_clone = pty.clone();
                fasync::Task::local(async move {
                    run_client(pty_clone, id, client_stream.cast_stream()).await;
                })
                .detach();
            }
            DeviceRequest::Close { responder } => {
                responder.send(Ok(())).or_else(ignore_peer_closed)?;
                break;
            }
            DeviceRequest::Query { responder } => {
                responder
                    .send(fpty::DeviceMarker::PROTOCOL_NAME.as_bytes())
                    .or_else(ignore_peer_closed)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn test_server_describe() {
        let pty = Pty::new();
        let event = pty.server.remote_event.duplicate_handle(zx::Rights::BASIC).unwrap();
        assert!(!event.is_invalid());
    }

    #[fuchsia::test]
    async fn test_open_client() {
        let mut pty = Pty::new();
        pty.create_client(0).unwrap();

        assert!(pty.clients.contains_key(&0));
        assert!(pty.active_id == Some(0));
        assert!(pty.control_id == Some(0));
    }

    #[fuchsia::test]
    async fn test_open_client_twice_fails() {
        let mut pty = Pty::new();
        pty.create_client(0).unwrap();
        assert_eq!(pty.create_client(0), Err(zx::Status::INVALID_ARGS));
    }

    #[fuchsia::test]
    async fn test_server_initial_signals() {
        let pty = Pty::new();
        let event = &pty.server.remote_event;

        let signals =
            event.wait_one(zx::Signals::USER_ALL, zx::MonotonicInstant::INFINITE_PAST).unwrap();
        assert!(signals.contains(zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits())));
        assert!(signals.contains(zx::Signals::from_bits_truncate(DeviceSignal::HANGUP.bits())));
    }

    #[fuchsia::test]
    async fn test_client_signals() {
        let mut pty = Pty::new();
        pty.create_client(0).unwrap();

        let client = pty.clients.get(&0).unwrap();
        let event = &client.remote_event;

        let signals =
            event.wait_one(zx::Signals::USER_ALL, zx::MonotonicInstant::INFINITE_PAST).unwrap();
        assert!(signals.contains(zx::Signals::from_bits_truncate(DeviceSignal::WRITABLE.bits())));
        assert!(!signals.contains(zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits())));
    }
}
