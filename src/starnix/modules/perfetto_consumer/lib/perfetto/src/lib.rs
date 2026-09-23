// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Increase recursion limit because LTO causes overflow.
#![recursion_limit = "256"]

use perfetto_protos::perfetto::protos::{
    DisableTracingRequest, EnableTracingRequest, FreeBuffersRequest, GetAsyncCommandRequest,
    GetAsyncCommandResponse, InitializeConnectionRequest, InitializeConnectionResponse, IpcFrame,
    ReadBuffersRequest, RegisterDataSourceRequest, RegisterDataSourceResponse, ipc_frame,
};
use prost::Message;
use starnix_core::task::{CurrentTask, EventHandler, Waiter};
use starnix_core::vfs::buffers::{VecInputBuffer, VecOutputBuffer};
use starnix_core::vfs::socket::{
    SocketDomain, SocketFile, SocketPeer, SocketProtocol, SocketType, resolve_unix_socket_address,
};
use starnix_core::vfs::{FileHandle, FsStr};

use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::vfs::FdEvents;
use std::collections::VecDeque;
use thiserror::Error;

/// A connection represents serializing Perfetto IPC requests to a file
/// Specifically, this is writing 4-byte-little-endian length prefixed proto-coded messages to the
/// receiver.
///
/// See https://perfetto.dev/docs/design-docs/api-and-abi for more details
pub struct IpcConnection {
    /// File to write to.
    file: FileHandle,
    /// Next unused request id. This is used for correlating replies to requests. We need to
    /// increment this after each request.
    request_id: u64,
}

#[derive(Error, Debug)]
pub enum IpcWriteError {
    #[error(transparent)]
    Encode(#[from] prost::EncodeError),
    #[error(transparent)]
    Write(#[from] Errno),
    #[error("TooLong: {0} exceeds max for u32")]
    TooLong(usize),
}

#[derive(Error, Debug)]
pub enum IpcReadError {
    #[error(transparent)]
    Decode(#[from] prost::DecodeError),
    #[error(transparent)]
    Read(#[from] Errno),
}

#[derive(Error, Debug)]
pub enum InvokeMethodError {
    #[error("could not not look up method name: {0}")]
    InvalidMethod(String),
    #[error(transparent)]
    IpcWrite(#[from] IpcWriteError),
    #[error(transparent)]
    IpcRead(#[from] IpcReadError),
    #[error("unexpected response: {0}")]
    InvalidResponse(String),
}

#[derive(Error, Debug)]
pub enum ProducerError {
    #[error(transparent)]
    InvokeMethod(#[from] InvokeMethodError),
    #[error(transparent)]
    IpcWrite(#[from] IpcWriteError),
    #[error(transparent)]
    IpcRead(#[from] IpcReadError),
    #[error("unexpected response: {0}")]
    InvalidResponse(String),
    #[error(transparent)]
    Decode(#[from] prost::DecodeError),
}

impl IpcConnection {
    pub fn new(file: FileHandle) -> Self {
        Self { file, request_id: 0 }
    }

    pub fn bind_service(
        &mut self,
        service_name: &str,
        current_task: &CurrentTask,
    ) -> Result<(), IpcWriteError> {
        // The first thing we need to send over our newly connected socket is send a request to
        // bind to the service.
        let bind_service_message = IpcFrame {
            request_id: Some(1),
            msg: Some(ipc_frame::Msg::MsgBindService(ipc_frame::BindService {
                service_name: Some(service_name.to_string()),
            })),
            ..Default::default()
        };
        self.write_frame(bind_service_message, current_task)
    }

    pub fn invoke_method(
        &mut self,
        service_id: u32,
        method_id: u32,
        arguments: Option<Vec<u8>>,
        current_task: &CurrentTask,
    ) -> Result<(), IpcWriteError> {
        let msg = IpcFrame {
            request_id: Some(self.allocate_request_id()),
            msg: Some(ipc_frame::Msg::MsgInvokeMethod(ipc_frame::InvokeMethod {
                service_id: Some(service_id),
                method_id: Some(method_id),
                args_proto: arguments,
                drop_reply: None,
            })),
            ..Default::default()
        };
        self.write_frame(msg, current_task)
    }

    fn write_frame(
        &mut self,
        frame: IpcFrame,
        current_task: &CurrentTask,
    ) -> Result<(), IpcWriteError> {
        // We need to length prefix our proto before encoding and sending it down the wire.
        let frame_len = u32::try_from(frame.encoded_len())
            .map_err(|_| IpcWriteError::TooLong(frame.encoded_len()))?;
        let mut bind_service_bytes =
            Vec::with_capacity(frame.encoded_len() + std::mem::size_of::<u32>());
        bind_service_bytes.extend_from_slice(&frame_len.to_le_bytes());
        frame.encode(&mut bind_service_bytes)?;
        let mut bind_service_buffer: VecInputBuffer = bind_service_bytes.into();
        self.file.write(current_task, &mut bind_service_buffer)?;
        Ok(())
    }

    // Perfetto requires a unique message id for each ipc request.
    fn allocate_request_id(&mut self) -> u64 {
        let id = self.request_id;
        self.request_id += 1;
        id
    }
}

/// State for reading Perfetto IPC frames.
///
/// Each frame is composed of a 32 bit length in little endian, followed by
/// the proto-encoded message. This state handles reads that only include
/// partial messages.
pub struct FrameReader {
    /// File to read from.
    file: FileHandle,
    /// Buffer for passing to read() calls.
    ///
    /// This buffer does not store any data long-term, but is persisted to
    /// avoid reallocating the buffer repeatedly.
    read_buffer: VecOutputBuffer,
    /// Data that has been read but not processed.
    data: VecDeque<u8>,
    /// If we've received enough bytes to know the next message's size, those
    /// bytes are removed from [data] and the size is populated here.
    next_message_size: Option<usize>,
}

impl FrameReader {
    pub fn new(file: FileHandle) -> Self {
        Self {
            file,
            read_buffer: VecOutputBuffer::new(4096),
            data: VecDeque::with_capacity(4096),
            next_message_size: None,
        }
    }

    /// Repeatedly reads from the specified file until a full message is available.
    pub fn next_frame_blocking(
        &mut self,
        current_task: &CurrentTask,
    ) -> Result<IpcFrame, IpcReadError> {
        loop {
            if self.next_message_size.is_none() && self.data.len() >= 4 {
                let len_bytes: [u8; 4] = self
                    .data
                    .drain(..4)
                    .collect::<Vec<_>>()
                    .try_into()
                    .expect("self.data has at least 4 elements");
                self.next_message_size = Some(u32::from_le_bytes(len_bytes) as usize);
            }
            if let Some(message_size) = self.next_message_size {
                if self.data.len() >= message_size {
                    let message: Vec<u8> = self.data.drain(..message_size).collect();
                    self.next_message_size = None;
                    return Ok(IpcFrame::decode(message.as_slice())?);
                }
            }

            let waiter = Waiter::new();
            self.file.wait_async(current_task, &waiter, FdEvents::POLLIN, EventHandler::None);
            while self.file.query_events(current_task)? & FdEvents::POLLIN != FdEvents::POLLIN {
                waiter.wait(current_task)?;
            }
            self.file.read(current_task, &mut self.read_buffer)?;
            self.data.extend(self.read_buffer.data());
            self.read_buffer.reset();
        }
    }
}

/// Bookkeeping information needed for IPC messages to and from Perfetto.
pub struct Consumer {
    /// File handle corresponding to the communication socket. Data is written to and read from
    /// this file.
    conn_file: FileHandle,
    /// State for combining read byte data into IPC frames.
    frame_reader: FrameReader,
    /// Reply from the BindService call that was made when the connection was opened.
    /// This call includes ids for the various IPCs that the Perfetto service supports.
    bind_service_reply: ipc_frame::BindServiceReply,
    /// Next unused request id. This is used for correlating repies to requests.
    request_id: u64,
}

impl Consumer {
    /// Opens a socket connection to the specified socket path and initializes the requisite
    /// bookkeeping information.
    pub fn new(current_task: &CurrentTask, socket_path: &FsStr) -> Result<Self, anyhow::Error> {
        let conn_file = SocketFile::new_socket(
            current_task,
            SocketDomain::Unix,
            SocketType::Stream,
            OpenFlags::RDWR,
            SocketProtocol::from_raw(0),
            /* kernel_private=*/ false,
        )?;
        let conn = SocketFile::get_from_file(&conn_file)?;
        let peer = SocketPeer::Handle(resolve_unix_socket_address(current_task, socket_path)?);
        conn.connect(current_task, peer)?;
        let mut frame_reader = FrameReader::new(conn_file.clone());
        let mut request_id = 1;

        let bind_service_message = IpcFrame {
            request_id: Some(request_id),
            data_for_testing: Vec::new(),
            msg: Some(ipc_frame::Msg::MsgBindService(ipc_frame::BindService {
                service_name: Some("ConsumerPort".to_string()),
            })),
        };
        request_id += 1;
        let mut bind_service_bytes =
            Vec::with_capacity(bind_service_message.encoded_len() + std::mem::size_of::<u32>());
        bind_service_bytes.extend_from_slice(
            &u32::try_from(bind_service_message.encoded_len()).unwrap().to_le_bytes(),
        );
        bind_service_message.encode(&mut bind_service_bytes)?;
        let mut bind_service_buffer: VecInputBuffer = bind_service_bytes.into();
        conn.file().write(current_task, &mut bind_service_buffer)?;

        let reply_frame = frame_reader.next_frame_blocking(current_task)?;

        let bind_service_reply = match reply_frame.msg {
            Some(ipc_frame::Msg::MsgBindServiceReply(reply)) => reply,
            m => return Err(anyhow::anyhow!("Got unexpected reply message: {:?}", m)),
        };

        Ok(Self { conn_file, frame_reader, bind_service_reply, request_id })
    }

    fn send_message(
        &mut self,
        current_task: &CurrentTask,
        msg: ipc_frame::Msg,
    ) -> Result<u64, anyhow::Error> {
        let request_id = self.request_id;
        let frame =
            IpcFrame { request_id: Some(request_id), data_for_testing: Vec::new(), msg: Some(msg) };

        self.request_id += 1;

        let mut frame_bytes = Vec::with_capacity(frame.encoded_len() + std::mem::size_of::<u32>());
        frame_bytes.extend_from_slice(&u32::try_from(frame.encoded_len())?.to_le_bytes());
        frame.encode(&mut frame_bytes)?;
        let mut buffer: VecInputBuffer = frame_bytes.into();
        self.conn_file.write(current_task, &mut buffer)?;

        Ok(request_id)
    }

    fn method_id(&self, name: &str) -> Result<u32, anyhow::Error> {
        for method in &self.bind_service_reply.methods {
            if let Some(method_name) = method.name.as_ref() {
                if method_name == name {
                    if let Some(id) = method.id {
                        return Ok(id);
                    } else {
                        return Err(anyhow::anyhow!(
                            "Matched method name {} but found no id",
                            method_name
                        ));
                    }
                }
            }
        }
        Err(anyhow::anyhow!("Did not find method {}", name))
    }

    pub fn enable_tracing(
        &mut self,
        current_task: &CurrentTask,
        req: EnableTracingRequest,
    ) -> Result<u64, anyhow::Error> {
        let method_id = self.method_id("EnableTracing")?;
        let mut encoded_args: Vec<u8> = Vec::with_capacity(req.encoded_len());
        req.encode(&mut encoded_args)?;

        self.send_message(
            current_task,
            ipc_frame::Msg::MsgInvokeMethod(ipc_frame::InvokeMethod {
                service_id: self.bind_service_reply.service_id,
                method_id: Some(method_id),
                args_proto: Some(encoded_args),
                drop_reply: None,
            }),
        )
    }

    pub fn disable_tracing(
        &mut self,
        current_task: &CurrentTask,
        req: DisableTracingRequest,
    ) -> Result<u64, anyhow::Error> {
        let method_id = self.method_id("DisableTracing")?;
        let mut encoded_args: Vec<u8> = Vec::with_capacity(req.encoded_len());
        req.encode(&mut encoded_args)?;

        self.send_message(
            current_task,
            ipc_frame::Msg::MsgInvokeMethod(ipc_frame::InvokeMethod {
                service_id: self.bind_service_reply.service_id,
                method_id: Some(method_id),
                args_proto: Some(encoded_args),
                drop_reply: None,
            }),
        )
    }

    pub fn read_buffers(
        &mut self,
        current_task: &CurrentTask,
        req: ReadBuffersRequest,
    ) -> Result<u64, anyhow::Error> {
        let method_id = self.method_id("ReadBuffers")?;
        let mut encoded_args: Vec<u8> = Vec::with_capacity(req.encoded_len());
        req.encode(&mut encoded_args)?;

        self.send_message(
            current_task,
            ipc_frame::Msg::MsgInvokeMethod(ipc_frame::InvokeMethod {
                service_id: self.bind_service_reply.service_id,
                method_id: Some(method_id),
                args_proto: Some(encoded_args),
                drop_reply: None,
            }),
        )
    }

    pub fn free_buffers(
        &mut self,
        current_task: &CurrentTask,
        req: FreeBuffersRequest,
    ) -> Result<u64, anyhow::Error> {
        let method_id = self.method_id("FreeBuffers")?;
        let mut encoded_args: Vec<u8> = Vec::with_capacity(req.encoded_len());
        req.encode(&mut encoded_args)?;

        self.send_message(
            current_task,
            ipc_frame::Msg::MsgInvokeMethod(ipc_frame::InvokeMethod {
                service_id: self.bind_service_reply.service_id,
                method_id: Some(method_id),
                args_proto: Some(encoded_args),
                drop_reply: None,
            }),
        )
    }

    pub fn next_frame_blocking(
        &mut self,
        current_task: &CurrentTask,
    ) -> Result<IpcFrame, IpcReadError> {
        self.frame_reader.next_frame_blocking(current_task)
    }
}

// A perfetto compatible producer that can communicate with perfetto over a socket.
// This struct provides a Rust compatible interface over the proto defined in
// //third_party/perfetto/protos/perfetto/ipc/producer_port.proto.
//
// See https://perfetto.dev/docs/design-docs/api-and-abi#socket-protocol for details about the
// protocol we implement here.
pub struct Producer {
    /// State for combining read byte data into IPC frames.
    frame_reader: FrameReader,

    /// Writer to write perfetto ipc to the socket
    ipc_connection: IpcConnection,

    /// After we connect, Perfetto will provide us with a service id which we will need to include
    /// in all our messages going forwards so that Perfetto can identify us.
    service_id: u32,

    // When we connect to the perfetto socket, it informs us which method names correspond to which
    // method ids. We save them here for reference when we need to invoke a method.
    method_map: std::collections::HashMap<String, u32>,
}

impl Producer {
    /// Opens a socket connection to the specified socket path and initializes the requisite
    /// bookkeeping information.
    pub fn new(current_task: &CurrentTask, socket: FileHandle) -> Result<Self, ProducerError> {
        let mut producer = Self {
            frame_reader: FrameReader::new(socket.clone()),
            ipc_connection: IpcConnection::new(socket),
            service_id: 0,
            method_map: std::collections::HashMap::new(),
        };

        // The first thing we need to send over our newly connected socket is send a request to
        // bind to the service.
        producer.ipc_connection.bind_service("ProducerPort", current_task)?;

        // Perfetto will then respond and tell us:
        // 1) Our service_id, which we'll need to include in future messages to identify ourself
        // 2) The list of methods that we can call using the "InvokeMethod" ipc
        let reply_frame = producer.frame_reader.next_frame_blocking(current_task)?;

        let ipc_frame::BindServiceReply { success, service_id, methods } = match reply_frame.msg {
            Some(ipc_frame::Msg::MsgBindServiceReply(reply)) => reply,
            m => {
                return Err(ProducerError::InvalidResponse(format!(
                    "Got unexpected reply message: {:?}",
                    m
                )));
            }
        };

        if !success.unwrap_or(false) {
            return Err(ProducerError::InvalidResponse("Bind to socket failed".into()));
        }

        // Build the possible methods we can call and save them for when we call InvokeMethod.
        producer.method_map = methods
            .into_iter()
            .flat_map(|ipc_frame::bind_service_reply::MethodInfo { id, name }| match (id, name) {
                (Some(id), Some(name)) => Some((name, id)),
                _ => None,
            })
            .collect();
        if let Some(service_id) = service_id {
            producer.service_id = service_id
        } else {
            return Err(ProducerError::InvalidResponse(
                "BindServiceReply did not include service_id".into(),
            ));
        }

        Ok(producer)
    }

    /// Called once only after establishing the connection with the Service.
    /// The service replies sending the shared memory file descriptor in reply.
    pub fn initialize_connection(
        &mut self,
        request: InitializeConnectionRequest,
        current_task: &CurrentTask,
    ) -> Result<InitializeConnectionResponse, ProducerError> {
        let (Some(reply), has_more) = self.invoke_method(
            "InitializeConnection",
            Some(request.encode_to_vec()),
            current_task,
        )?
        else {
            return Err(ProducerError::InvalidResponse("expected a response but got none".into()));
        };
        if has_more {
            return Err(ProducerError::InvalidResponse(
                "InitializeConnection should not stream but got a streaming response".into(),
            ));
        }
        Ok(InitializeConnectionResponse::decode(reply.as_ref())?)
    }

    /// Advertises a new data source.
    pub fn register_data_source(
        &mut self,
        request: RegisterDataSourceRequest,
        current_task: &CurrentTask,
    ) -> Result<RegisterDataSourceResponse, ProducerError> {
        let (Some(reply), has_more) =
            self.invoke_method("RegisterDataSource", Some(request.encode_to_vec()), current_task)?
        else {
            return Err(ProducerError::InvalidResponse(
                "RegisterDataSource expected a response but got none".into(),
            ));
        };
        if has_more {
            return Err(ProducerError::InvalidResponse(
                "RegisterDataSource should not stream but got a streaming response".into(),
            ));
        }
        Ok(RegisterDataSourceResponse::decode(reply.as_ref())?)
    }

    /// Invoke a named method and block until the service replies.
    fn invoke_method(
        &mut self,
        method_name: &str,
        arguments: Option<Vec<u8>>,
        current_task: &CurrentTask,
    ) -> Result<(Option<Vec<u8>>, bool), InvokeMethodError> {
        self.invoke_method_inner(method_name, arguments, current_task)?;

        let reply_frame = self.frame_reader.next_frame_blocking(current_task)?;

        let ipc_frame::InvokeMethodReply { success, has_more, reply_proto } = match reply_frame.msg
        {
            Some(ipc_frame::Msg::MsgInvokeMethodReply(reply)) => reply,
            m => {
                return Err(InvokeMethodError::InvalidResponse(format!(
                    "unexpected reply message: {:?}",
                    m
                )));
            }
        };
        match success {
            Some(true) => Ok((reply_proto, has_more.unwrap_or(false))),
            _ => {
                return Err(InvokeMethodError::InvalidResponse(format!(
                    "InvokeMethod Reply did not succeed. Reply: success: {:?}, has_more: {:?}, proto: {:?}",
                    success, has_more, reply_proto,
                )));
            }
        }
    }

    /// Invoke a named method without blocking for a response.
    fn invoke_method_inner(
        &mut self,
        method_name: &str,
        arguments: Option<Vec<u8>>,
        current_task: &CurrentTask,
    ) -> Result<(), InvokeMethodError> {
        let Some(method_id) = self.method_map.get(method_name).copied() else {
            return Err(InvokeMethodError::InvalidMethod(method_name.into()));
        };
        self.ipc_connection.invoke_method(self.service_id, method_id, arguments, current_task)?;
        Ok(())
    }

    /// This is a backchannel to get asynchronous commands / notifications back
    /// from the Service.
    pub fn get_command_request(&mut self, current_task: &CurrentTask) -> Result<(), ProducerError> {
        Ok(self.invoke_method_inner(
            "GetAsyncCommand",
            Some(GetAsyncCommandRequest {}.encode_to_vec()),
            current_task,
        )?)
    }

    /// After calling get_command_request, block until a response can be read.
    pub fn get_command_response(
        &mut self,
        current_task: &CurrentTask,
    ) -> Result<(Option<GetAsyncCommandResponse>, bool), ProducerError> {
        let reply_frame = self.frame_reader.next_frame_blocking(current_task)?;

        let ipc_frame::InvokeMethodReply { success, has_more, reply_proto } = match reply_frame.msg
        {
            Some(ipc_frame::Msg::MsgInvokeMethodReply(reply)) => reply,
            m => {
                return Err(ProducerError::InvalidResponse(format!(
                    "Got unexpected reply message: {:?}",
                    m
                )));
            }
        };
        if !success.unwrap_or(false) {
            return Err(ProducerError::InvalidResponse(format!(
                "InvokeMethod Reply did not include success. Reply: success: {:?}, has_more: {:?}, proto: {:?}",
                success, has_more, reply_proto
            )));
        }

        let Some(reply_proto) = reply_proto else {
            return Err(ProducerError::InvalidResponse(
                "InvokeMethod reply didn't include a proto".into(),
            ));
        };
        Ok((
            Some(GetAsyncCommandResponse::decode(reply_proto.as_ref())?),
            has_more.unwrap_or(false),
        ))
    }
}
