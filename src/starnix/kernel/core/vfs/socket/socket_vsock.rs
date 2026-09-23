// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::{CurrentTask, EventHandler, WaitCanceler, WaitQueue, Waiter};
use crate::vfs::FileHandle;
use crate::vfs::buffers::{AncillaryData, InputBuffer, MessageReadInfo, OutputBuffer};
use crate::vfs::socket::{
    AcceptQueue, DEFAULT_LISTEN_BACKLOG, Socket, SocketAddress, SocketDomain, SocketHandle,
    SocketMessageFlags, SocketOps, SocketPeer, SocketProtocol, SocketShutdownFlags, SocketType,
};
use starnix_sync::{LockDepGuard, LockDepMutex, VsockSocketInnerLock, allow_subclass};
use starnix_uapi::auth::Credentials;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{errno, error, ucred};

// An implementation of AF_VSOCK.
// See https://man7.org/linux/man-pages/man7/vsock.7.html

pub struct VsockSocket {
    inner: LockDepMutex<VsockSocketInner, VsockSocketInnerLock>,
}

struct VsockSocketInner {
    /// The address that this socket has been bound to, if it has been bound.
    address: Option<SocketAddress>,

    // WaitQueue for listening sockets.
    waiters: WaitQueue,

    // state of the vsock. Contains a handle to a ZxioBackedSocket when connected.
    state: VsockSocketState,
}

enum VsockSocketState {
    /// The socket has not been connected.
    Disconnected,

    /// The socket has had `listen` called and can accept incoming connections.
    Listening(AcceptQueue),

    /// The socket is connected to a ZxioBackedSocket.
    Connected { file: FileHandle, peer_addr: SocketAddress },

    /// The socket is closed.
    Closed,
}

fn downcast_socket_to_vsock(socket: &Socket) -> &VsockSocket {
    // It is a programing error if we are downcasting
    // a different type of socket as sockets from different families
    // should not communicate, so unwrapping here
    // will let us know that.
    socket.downcast_socket::<VsockSocket>().unwrap()
}

impl VsockSocket {
    pub fn new(_socket_type: SocketType) -> VsockSocket {
        VsockSocket {
            inner: VsockSocketInner {
                address: None,
                waiters: WaitQueue::default(),
                state: VsockSocketState::Disconnected,
            }
            .into(),
        }
    }

    /// Locks and returns the inner state of the Socket.
    fn lock(&self) -> LockDepGuard<'_, VsockSocketInner> {
        self.inner.lock()
    }
}

impl SocketOps for VsockSocket {
    // Connect with Vsock sockets is not allowed as
    // we only connect from the enclosing OK.
    fn connect(
        &self,
        _socket: &SocketHandle,
        _current_task: &CurrentTask,
        _peer: SocketPeer,
    ) -> Result<(), Errno> {
        error!(EPROTOTYPE)
    }

    fn listen(&self, _socket: &Socket, backlog: i32, _credentials: ucred) -> Result<(), Errno> {
        let mut inner = self.lock();
        let is_bound = inner.address.is_some();
        let backlog = if backlog < 0 { DEFAULT_LISTEN_BACKLOG } else { backlog as usize };
        match &mut inner.state {
            VsockSocketState::Disconnected if is_bound => {
                inner.state = VsockSocketState::Listening(AcceptQueue::new(backlog));
                Ok(())
            }
            VsockSocketState::Listening(queue) => {
                queue.set_backlog(backlog)?;
                Ok(())
            }
            _ => error!(EINVAL),
        }
    }

    fn accept(&self, socket: &Socket, _current_task: &CurrentTask) -> Result<SocketHandle, Errno> {
        match socket.socket_type {
            SocketType::Stream | SocketType::SeqPacket => {}
            _ => return error!(EOPNOTSUPP),
        }
        let mut inner = self.lock();
        let queue = match &mut inner.state {
            VsockSocketState::Listening(queue) => queue,
            _ => return error!(EINVAL),
        };
        let socket = queue.sockets.pop_front().ok_or_else(|| errno!(EAGAIN))?;
        Ok(socket)
    }

    fn bind(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        socket_address: SocketAddress,
    ) -> Result<(), Errno> {
        match socket_address {
            SocketAddress::Vsock { .. } => {}
            _ => return error!(EINVAL),
        }
        let mut inner = self.lock();
        if inner.address.is_some() {
            return error!(EINVAL);
        }
        inner.address = Some(socket_address);
        Ok(())
    }

    fn read(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
        _flags: SocketMessageFlags,
    ) -> Result<MessageReadInfo, Errno> {
        let (address, file) = {
            let inner = self.lock();
            let address = inner.address.clone();

            match &inner.state {
                VsockSocketState::Connected { file, .. } => (address, file.clone()),
                _ => return error!(EBADF),
            }
        };
        let bytes_read =
            current_task.override_creds(Credentials::root(), || file.read(current_task, data))?;
        Ok(MessageReadInfo {
            bytes_read,
            message_length: bytes_read,
            address,
            ancillary_data: vec![],
        })
    }

    fn write(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        data: &mut dyn InputBuffer,
        _dest_address: &mut Option<SocketAddress>,
        _ancillary_data: &mut Vec<AncillaryData>,
    ) -> Result<usize, Errno> {
        let file = {
            let inner = self.lock();
            match &inner.state {
                VsockSocketState::Connected { file, .. } => file.clone(),
                _ => return error!(EBADF),
            }
        };
        current_task.override_creds(Credentials::root(), || file.write(current_task, data))
    }

    fn wait_async(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        let inner = self.lock();
        match &inner.state {
            VsockSocketState::Connected { file, .. } => file
                .wait_async(current_task, waiter, events, handler)
                .expect("vsock socket should be connected to a file that can be waited on"),
            _ => inner.waiters.wait_async_fd_events(waiter, events, handler),
        }
    }

    fn query_events(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        self.lock().query_events(current_task)
    }

    fn shutdown(&self, _socket: &Socket, _how: SocketShutdownFlags) -> Result<(), Errno> {
        self.lock().state = VsockSocketState::Closed;
        Ok(())
    }

    fn close(&self, _current_task: &CurrentTask, socket: &Socket) {
        // Call to shutdown should never fail, so unwrap is OK
        self.shutdown(socket, SocketShutdownFlags::READ | SocketShutdownFlags::WRITE).unwrap();
    }

    fn getsockname(&self, socket: &Socket) -> Result<SocketAddress, Errno> {
        let inner = self.lock();
        if let Some(address) = &inner.address {
            Ok(address.clone())
        } else {
            Ok(SocketAddress::default_for_domain(socket.domain))
        }
    }

    fn getpeername(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        let inner = self.lock();
        match &inner.state {
            VsockSocketState::Connected { peer_addr, .. } => Ok(peer_addr.clone()),
            _ => {
                error!(ENOTCONN)
            }
        }
    }
}

impl VsockSocket {
    pub fn remote_connection(
        &self,
        socket: &Socket,
        current_task: &CurrentTask,
        file: FileHandle,
    ) -> Result<(), Errno> {
        // we only allow non-blocking files here, so that
        // read and write on file can return EAGAIN.
        assert!(file.flags().contains(OpenFlags::NONBLOCK));
        if socket.socket_type != SocketType::Stream {
            return error!(ENOTSUP);
        }
        if socket.domain != SocketDomain::Vsock {
            return error!(EINVAL);
        }

        let mut inner = self.lock();
        match &mut inner.state {
            VsockSocketState::Listening(queue) => {
                if queue.sockets.len() >= queue.backlog {
                    return error!(EAGAIN);
                }
                let remote_socket = Socket::new(
                    current_task,
                    SocketDomain::Vsock,
                    SocketType::Stream,
                    SocketProtocol::default(),
                    /* kernel_private = */ true,
                )?;
                // The lock on remote_socket is safe because the socket is not shared with any other
                // thread yet.
                let _token = allow_subclass();
                downcast_socket_to_vsock(&remote_socket).lock().state =
                    VsockSocketState::Connected {
                        file,
                        peer_addr: SocketAddress::Vsock {
                            port: u32::MAX,
                            cid: starnix_uapi::VMADDR_CID_HOST,
                        },
                    };
                queue.sockets.push_back(remote_socket);
                inner.waiters.notify_fd_events(FdEvents::POLLIN);
                Ok(())
            }
            _ => error!(EINVAL),
        }
    }
}

impl VsockSocketInner {
    fn query_events(&self, current_task: &CurrentTask) -> Result<FdEvents, Errno> {
        Ok(match &self.state {
            VsockSocketState::Disconnected => FdEvents::empty(),
            VsockSocketState::Connected { file, .. } => current_task
                .override_creds(Credentials::root(), || file.query_events(current_task))?,
            VsockSocketState::Listening(queue) => {
                if !queue.sockets.is_empty() {
                    FdEvents::POLLIN
                } else {
                    FdEvents::empty()
                }
            }
            VsockSocketState::Closed => FdEvents::POLLHUP,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::fuchsia::create_fuchsia_pipe;
    use crate::mm::PAGE_SIZE;
    use crate::task::dynamic_thread_spawner::SpawnRequestBuilder;
    use crate::testing::spawn_kernel_and_run;
    use crate::vfs::EpollFileObject;
    use crate::vfs::buffers::{VecInputBuffer, VecOutputBuffer};
    use crate::vfs::socket::SocketFile;
    use futures::executor::block_on;
    use starnix_uapi::vfs::EpollEvent;
    use syncio::Zxio;

    #[::fuchsia::test]
    async fn test_vsock_socket() {
        spawn_kernel_and_run(async |current_task| {
            let (fs1, fs2) = fidl::Socket::create_stream();
            const VSOCK_PORT: u32 = 5555;

            let listen_socket = Socket::new(
                &current_task,
                SocketDomain::Vsock,
                SocketType::Stream,
                SocketProtocol::default(),
                /* kernel_private = */ false,
            )
            .expect("Failed to create socket.");
            let vsock_ns = current_task.running_state().abstract_vsock_namespace.clone();
            vsock_ns
                .bind(&current_task, VSOCK_PORT, &listen_socket)
                .expect("Failed to bind socket.");
            listen_socket.listen(&current_task, 10).expect("Failed to listen.");

            let listen_socket =
                vsock_ns.lookup(&VSOCK_PORT).expect("Failed to look up listening socket.");
            let remote =
                create_fuchsia_pipe(&current_task, fs2, OpenFlags::RDWR | OpenFlags::NONBLOCK)
                    .unwrap();
            listen_socket
                .downcast_socket::<VsockSocket>()
                .unwrap()
                .remote_connection(&listen_socket, &current_task, remote)
                .unwrap();

            let server_socket = listen_socket.accept(&current_task).unwrap();

            let test_bytes_in: [u8; 5] = [0, 1, 2, 3, 4];
            assert_eq!(fs1.write(&test_bytes_in[..]).unwrap(), test_bytes_in.len());
            let mut buffer_iterator = VecOutputBuffer::new(*PAGE_SIZE as usize);
            let read_message_info = server_socket
                .read(&current_task, &mut buffer_iterator, SocketMessageFlags::empty())
                .unwrap();
            assert_eq!(read_message_info.bytes_read, test_bytes_in.len());
            assert_eq!(buffer_iterator.data(), test_bytes_in);

            let test_bytes_out: [u8; 10] = [9, 8, 7, 6, 5, 4, 3, 2, 1, 0];
            let mut buffer_iterator = VecInputBuffer::new(&test_bytes_out);
            server_socket
                .write(&current_task, &mut buffer_iterator, &mut None, &mut vec![])
                .unwrap();
            assert_eq!(buffer_iterator.bytes_read(), test_bytes_out.len());

            let mut read_back_buf = [0u8; 100];
            assert_eq!(test_bytes_out.len(), fs1.read(&mut read_back_buf).unwrap());
            assert_eq!(&read_back_buf[..test_bytes_out.len()], &test_bytes_out);

            server_socket.close(&current_task);
            listen_socket.close(&current_task);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_vsock_write_while_read() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let (fs1, fs2) = fidl::Socket::create_stream();
            let socket = Socket::new(
                &current_task,
                SocketDomain::Vsock,
                SocketType::Stream,
                SocketProtocol::default(),
                /* kernel_private = */ false,
            )
            .expect("Failed to create socket.");
            let remote =
                create_fuchsia_pipe(&current_task, fs2, OpenFlags::RDWR | OpenFlags::NONBLOCK)
                    .unwrap();
            downcast_socket_to_vsock(&socket).lock().state = VsockSocketState::Connected {
                file: remote,
                peer_addr: SocketAddress::Vsock {
                    port: u32::MAX,
                    cid: starnix_uapi::VMADDR_CID_HOST,
                },
            };
            let socket_file =
                SocketFile::from_socket(&current_task, socket, OpenFlags::RDWR, false)
                    .expect("Failed to create socket file.");

            const XFER_SIZE: usize = 42;

            let socket_clone = socket_file.clone();
            let closure = move |current_task: &CurrentTask| {
                let bytes_read =
                    socket_clone.read(current_task, &mut VecOutputBuffer::new(XFER_SIZE)).unwrap();
                assert_eq!(XFER_SIZE, bytes_read);
            };
            let (result, req) =
                SpawnRequestBuilder::new().with_sync_closure(closure).build_with_async_result();
            kernel.kthreads.spawner().spawn_from_request(req);

            // Wait for the thread to become blocked on the read.
            std::thread::sleep(std::time::Duration::from_secs(2));

            socket_file.write(&current_task, &mut VecInputBuffer::new(&[0; XFER_SIZE])).unwrap();

            let mut buffer = [0u8; 1024];
            assert_eq!(XFER_SIZE, fs1.read(&mut buffer).unwrap());
            assert_eq!(XFER_SIZE, fs1.write(&buffer[..XFER_SIZE]).unwrap());
            block_on(result).unwrap();
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_vsock_poll() {
        spawn_kernel_and_run(async |current_task| {
            let (client, server) = zx::Socket::create_stream();
            let pipe = create_fuchsia_pipe(&current_task, client, OpenFlags::RDWR)
                .expect("create_fuchsia_pipe");
            let server_zxio = Zxio::create(server.into_handle()).expect("Zxio::create");
            let socket_object = Socket::new(
                &current_task,
                SocketDomain::Vsock,
                SocketType::Stream,
                SocketProtocol::default(),
                /* kernel_private = */ false,
            )
            .expect("Failed to create socket.");
            downcast_socket_to_vsock(&socket_object).lock().state = VsockSocketState::Connected {
                file: pipe,
                peer_addr: SocketAddress::Vsock {
                    port: u32::MAX,
                    cid: starnix_uapi::VMADDR_CID_HOST,
                },
            };
            let socket = SocketFile::from_socket(
                &current_task,
                socket_object.clone(),
                OpenFlags::RDWR,
                false,
            )
            .expect("Failed to create socket file.");

            assert_eq!(
                socket.query_events(&current_task),
                Ok(FdEvents::POLLOUT | FdEvents::POLLWRNORM)
            );

            let epoll_object = EpollFileObject::new_file(&current_task);
            let epoll_file = epoll_object.downcast_file::<EpollFileObject>().unwrap();
            let event = EpollEvent::new(FdEvents::POLLIN, 0);
            epoll_file.add(&current_task, &socket, &epoll_object, event).expect("poll_file.add");

            let fds = epoll_file.wait(&current_task, 1, zx::MonotonicInstant::ZERO).expect("wait");
            assert!(fds.is_empty());

            assert_eq!(server_zxio.write(&[0]).expect("write"), 1);

            assert_eq!(
                socket.query_events(&current_task),
                Ok(FdEvents::POLLOUT
                    | FdEvents::POLLWRNORM
                    | FdEvents::POLLIN
                    | FdEvents::POLLRDNORM)
            );
            let fds = epoll_file.wait(&current_task, 1, zx::MonotonicInstant::ZERO).expect("wait");
            assert_eq!(fds.len(), 1);

            assert_eq!(socket.read(&current_task, &mut VecOutputBuffer::new(64)).expect("read"), 1);

            assert_eq!(
                socket.query_events(&current_task),
                Ok(FdEvents::POLLOUT | FdEvents::POLLWRNORM)
            );
            let fds = epoll_file.wait(&current_task, 1, zx::MonotonicInstant::ZERO).expect("wait");
            assert!(fds.is_empty());
        })
        .await;
    }
}
