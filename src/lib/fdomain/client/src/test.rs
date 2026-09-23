// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::channel::HandleOp;
use crate::{
    AnyHandle, AsHandleRef, Client, Error, FDomainTransport, HandleBased, OnFDomainSignals, Peered,
    Socket, VmoOptions,
};
use fdomain_container::FDomain;
use fdomain_container::wire::FDomainCodec;
use fidl_fuchsia_fdomain::Error as FDomainError;
use fuchsia_sync::Mutex;
use futures::stream::Stream;
use futures::{AsyncReadExt, FutureExt, StreamExt};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker, ready};

#[derive(Clone, Debug)]
struct TestError(String);

impl std::error::Error for TestError {}

impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

struct TestFDomain(FDomainCodec, Arc<Mutex<FaultInjectorState>>);

impl TestFDomain {
    fn new_client() -> (Arc<Client>, FaultInjector) {
        let (client, fault_injector, fut) = TestFDomain::new_client_and_fut();
        fuchsia_async::Task::spawn(fut).detach();
        (client, fault_injector)
    }

    fn new_client_and_fut() -> (Arc<Client>, FaultInjector, impl std::future::Future<Output = ()>) {
        let fault_injector =
            FaultInjector(Arc::new(Mutex::new(FaultInjectorState::SendBadMessages(Vec::new()))));
        let (client, fut) = Client::new(TestFDomain(
            FDomainCodec::new(FDomain::new_empty()),
            Arc::clone(&fault_injector.0),
        ));
        (client, fault_injector, fut)
    }
}

impl FDomainTransport for TestFDomain {
    fn poll_send_message(
        mut self: Pin<&mut Self>,
        msg: &[u8],
        _ctx: &mut Context<'_>,
    ) -> Poll<Result<(), Option<std::io::Error>>> {
        Poll::Ready(self.0.message(msg).map_err(|x| Some(std::io::Error::other(x))))
    }
}

impl Stream for TestFDomain {
    type Item = Result<Box<[u8]>, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        {
            let mut faults = self.1.lock();
            match &mut *faults {
                FaultInjectorState::Error(e) => {
                    return Poll::Ready(Some(Err(std::io::Error::other(e.clone()))));
                }

                FaultInjectorState::SendBadMessages(f) => {
                    if let Some(send) = f.pop() {
                        return Poll::Ready(Some(Ok(send)));
                    }
                }
            }
        }

        Pin::new(&mut self.0).poll_next(cx).map_err(std::io::Error::other)
    }
}

enum FaultInjectorState {
    Error(TestError),
    SendBadMessages(Vec<Box<[u8]>>),
}

struct FaultInjector(Arc<Mutex<FaultInjectorState>>);

impl FaultInjector {
    fn inject(&self, e: TestError) {
        *self.0.lock() = FaultInjectorState::Error(e)
    }

    fn send_garbage(&self, g: impl AsRef<[u8]>) {
        let mut f = self.0.lock();
        let FaultInjectorState::SendBadMessages(queue) = &mut *f else {
            panic!("Injected failure then still tried to send garbage!");
        };
        queue.insert(0, Box::from(g.as_ref()));
    }
}

#[fuchsia::test]
async fn socket() {
    let (client, _) = TestFDomain::new_client();

    let (a, b) = client.create_stream_socket();
    const TEST_STR: &[u8] = b"Feral Cats Move In Mysterious Ways";

    a.fdomain_write_all(TEST_STR).await.unwrap();

    let mut got = Vec::with_capacity(TEST_STR.len());
    let mut buf = [0u8; TEST_STR.len()];

    while got.len() < TEST_STR.len() {
        let new_bytes = b.fdomain_read(&mut buf).await.unwrap();
        got.extend_from_slice(&buf[..new_bytes]);
    }

    assert_eq!(TEST_STR, got.as_slice());
}

#[fuchsia::test]
async fn datagram_socket() {
    let (client, _) = TestFDomain::new_client();

    let (a, b) = client.create_datagram_socket();
    const TEST_STR_1: &[u8] = b"Feral Cats Move In Mysterious Ways";
    const TEST_STR_2: &[u8] = b"Joyous Throbbing! Jubilant Pulsing!";

    a.fdomain_write_all(TEST_STR_1).await.unwrap();
    a.fdomain_write_all(TEST_STR_2).await.unwrap();

    let mut buf = [0u8; 2];

    let new_bytes = b.fdomain_read(&mut buf).await.unwrap();
    assert_eq!(new_bytes, 2);
    assert_eq!(&TEST_STR_1[..2], &buf);

    let new_bytes = b.fdomain_read(&mut buf).await.unwrap();
    assert_eq!(new_bytes, 2);
    assert_eq!(&TEST_STR_2[..2], &buf);
}

#[fuchsia::test]
async fn datagram_socket_underflow() {
    let (client, _) = TestFDomain::new_client();

    let (a, b) = client.create_datagram_socket();
    const TEST_STR_1: &[u8] = b"Feral Cats Move In Mysterious Ways";
    const TEST_STR_2: &[u8] = b"Joyous Throbbing! Jubilant Pulsing!";

    a.fdomain_write_all(TEST_STR_1).await.unwrap();
    a.fdomain_write_all(TEST_STR_2).await.unwrap();

    const MAX_LEN: usize =
        if TEST_STR_1.len() > TEST_STR_2.len() { TEST_STR_1.len() } else { TEST_STR_2.len() };
    let mut buf = [0u8; MAX_LEN * 2];

    let new_bytes = b.fdomain_read(&mut buf).await.unwrap();
    assert_eq!(new_bytes, TEST_STR_1.len());
    assert_eq!(TEST_STR_1, &buf[..TEST_STR_1.len()]);

    let new_bytes = b.fdomain_read(&mut buf).await.unwrap();
    assert_eq!(new_bytes, TEST_STR_2.len());
    assert_eq!(TEST_STR_2, &buf[..TEST_STR_2.len()]);
}

#[fuchsia::test]
async fn channel() {
    let (client, _) = TestFDomain::new_client();

    let (a, b) = client.create_channel();
    let (c, d) = client.create_stream_socket();
    const TEST_STR_1: &[u8] = b"Feral Cats Move In Mysterious Ways";
    const TEST_STR_2: &[u8] = b"Joyous Throbbing! Jubilant Pulsing!";

    a.fdomain_write(TEST_STR_1, vec![c.into()]).await.unwrap();
    d.fdomain_write_all(TEST_STR_2).await.unwrap();

    let mut msg = b.recv_msg().await.unwrap();

    assert_eq!(TEST_STR_1, msg.bytes.as_slice());

    let handle = msg.handles.pop().unwrap();
    assert!(msg.handles.is_empty());

    let expect_rights = fidl::Rights::DUPLICATE
        | fidl::Rights::TRANSFER
        | fidl::Rights::READ
        | fidl::Rights::WRITE
        | fidl::Rights::GET_PROPERTY
        | fidl::Rights::SET_PROPERTY
        | fidl::Rights::SIGNAL
        | fidl::Rights::SIGNAL_PEER
        | fidl::Rights::WAIT
        | fidl::Rights::INSPECT
        | fidl::Rights::MANAGE_SOCKET;
    assert_eq!(expect_rights, handle.rights);

    let AnyHandle::Socket(e) = handle.handle else { panic!() };

    let mut got = Vec::with_capacity(TEST_STR_2.len());
    let mut buf = [0u8; TEST_STR_2.len()];

    while got.len() < TEST_STR_2.len() {
        let new_bytes = e.fdomain_read(&mut buf).await.unwrap();
        got.extend_from_slice(&buf[..new_bytes]);
    }

    assert_eq!(TEST_STR_2, got.as_slice());
}

#[fuchsia::test]
async fn socket_async() {
    let (client, fault_injector) = TestFDomain::new_client();

    let (a, b) = client.create_stream_socket();
    const TEST_STR_A: &[u8] = b"Feral Cats Move In Mysterious Ways";
    const TEST_STR_B: &[u8] = b"Almost all of our feelings were programmed in to us.";

    let (mut b_reader, b_writer) = b.stream().unwrap();
    b_writer.write_all(TEST_STR_A).await.unwrap();

    let write_side = async move {
        let mut got = Vec::with_capacity(TEST_STR_A.len());
        let mut buf = [0u8; TEST_STR_A.len()];

        while got.len() < TEST_STR_A.len() {
            let new_bytes = a.fdomain_read(&mut buf).await.unwrap();
            got.extend_from_slice(&buf[..new_bytes]);
        }

        assert_eq!(TEST_STR_A, got.as_slice());

        for _ in 0..5 {
            a.fdomain_write_all(TEST_STR_A).await.unwrap();
            fuchsia_async::Timer::new(std::time::Duration::from_millis(10)).await;
            a.fdomain_write_all(TEST_STR_B).await.unwrap();
            fuchsia_async::Timer::new(std::time::Duration::from_millis(10)).await;
        }

        fault_injector.inject(TestError("Connection failed".to_owned()));

        let err = a.fdomain_write_all(TEST_STR_A).await.unwrap_err();
        let Error::Transport(Some(err)) = err else { panic!("Wrong error type!") };

        let TestError(err) = err.get_ref().unwrap().downcast_ref().unwrap();
        assert_eq!("Connection failed", err);
    };

    let read_side = async move {
        let mut buf = Vec::new();
        buf.resize((TEST_STR_A.len() + TEST_STR_B.len()) * 5, 0);

        for mut buf in buf.chunks_mut(20) {
            while !buf.is_empty() {
                let len = b_reader.fdomain_read(buf).await.unwrap();
                buf = &mut buf[len..];
            }
        }

        let err = b_reader.fdomain_read(&mut [0]).await.unwrap_err();
        let Error::Transport(Some(err)) = err else { panic!("Wrong error type!") };

        let TestError(err) = err.get_ref().unwrap().downcast_ref().unwrap();
        assert_eq!("Connection failed", err);

        let mut buf = buf.as_mut_slice();

        for _ in 0..5 {
            assert!(buf.starts_with(TEST_STR_A));
            buf = &mut buf[TEST_STR_A.len()..];
            assert!(buf.starts_with(TEST_STR_B));
            buf = &mut buf[TEST_STR_B.len()..];
        }

        assert!(buf.is_empty());
    };

    futures::future::join(read_side, write_side).await;
}

#[fuchsia::test]
async fn socket_drop_read_fut() {
    let (client, _fault_injector) = TestFDomain::new_client();

    let mut buf1 = [0u8; 2];
    let mut buf2 = [0u8; 2];
    let mut buf3 = [0u8; 2];

    let (a, b) = client.create_stream_socket();
    let mut fut1 = b.fdomain_read(&mut buf1);
    let mut fut2 = b.fdomain_read(&mut buf2);
    let mut fut3 = b.fdomain_read(&mut buf3);

    let mut null_cx = std::task::Context::from_waker(&std::task::Waker::noop());
    assert!(fut1.poll_unpin(&mut null_cx).is_pending());
    assert!(fut2.poll_unpin(&mut null_cx).is_pending());
    assert!(fut3.poll_unpin(&mut null_cx).is_pending());

    a.fdomain_write_all(b"abcd").await.unwrap();

    assert_eq!(2, fut1.await.unwrap());
    std::mem::drop(fut2);
    assert_eq!(2, fut3.await.unwrap());

    assert_eq!(b"ab", &buf1);
    assert_eq!(b"cd", &buf3);
}

#[fuchsia::test]
async fn channel_async() {
    let (client, fault_injector) = TestFDomain::new_client();

    let (a, b) = client.create_channel();
    let test_str_a = b"Feral Cats Move In Mysterious Ways";
    let test_str_b = b"Almost all of our feelings were programmed in to us.";

    let (mut b_reader, b_writer) = b.stream().unwrap();
    b_writer.fdomain_write(test_str_a, Vec::new()).await.unwrap();

    let write_side = async move {
        let msg = a.recv_msg().await.unwrap();

        assert_eq!(test_str_a, msg.bytes.as_slice());
        assert!(msg.handles.is_empty());

        for _ in 0..5 {
            a.fdomain_write(test_str_a, Vec::new()).await.unwrap();
            fuchsia_async::Timer::new(std::time::Duration::from_millis(10)).await;
            a.fdomain_write(test_str_b, Vec::new()).await.unwrap();
            fuchsia_async::Timer::new(std::time::Duration::from_millis(10)).await;
        }

        fault_injector.inject(TestError("Connection failed".to_owned()));

        let err = a.fdomain_write(test_str_a, Vec::new()).await.unwrap_err();
        let Error::Transport(Some(err)) = err else { panic!("Wrong error type!") };

        let TestError(err) = err.get_ref().unwrap().downcast_ref().unwrap();
        assert_eq!("Connection failed", err);
    };

    let read_side = async move {
        let mut msgs = Vec::new();

        for _ in 0..10 {
            msgs.push(b_reader.next().await.unwrap().unwrap());
        }

        let err = b_reader.next().await.unwrap().unwrap_err();
        let Error::Transport(Some(err)) = err else { panic!("Wrong error type!") };

        let TestError(err) = err.get_ref().unwrap().downcast_ref().unwrap();
        assert_eq!("Connection failed", err);

        for pair in msgs.chunks(2) {
            let a = &pair[0];
            let b = &pair[1];
            assert_eq!(test_str_a, a.bytes.as_slice());
            assert_eq!(test_str_b, b.bytes.as_slice());
            assert!(a.handles.is_empty());
            assert!(b.handles.is_empty());
        }
    };

    futures::future::join(read_side, write_side).await;
}

#[fuchsia::test]
async fn channel_async_shutdown() {
    let (client, _fault_injector) = TestFDomain::new_client();

    let (a, b) = client.create_channel();
    let test_str_a = b"Feral Cats Move In Mysterious Ways";
    let test_str_b = b"Almost all of our feelings were programmed in to us.";

    let (mut b_reader, b_writer) = b.stream().unwrap();
    b_writer.fdomain_write(test_str_a, Vec::new()).await.unwrap();

    let write_side = async move {
        let msg = a.recv_msg().await.unwrap();

        assert_eq!(test_str_a, msg.bytes.as_slice());
        assert!(msg.handles.is_empty());

        for _ in 0..5 {
            a.fdomain_write(test_str_a, Vec::new()).await.unwrap();
            fuchsia_async::Timer::new(std::time::Duration::from_millis(10)).await;
            a.fdomain_write(test_str_b, Vec::new()).await.unwrap();
            fuchsia_async::Timer::new(std::time::Duration::from_millis(10)).await;
        }

        std::mem::drop(a);
    };

    let read_side = async move {
        let mut msgs = Vec::new();

        for _ in 0..10 {
            msgs.push(b_reader.next().await.unwrap().unwrap());
        }

        let err = b_reader.next().await.unwrap().unwrap_err();
        let Error::FDomain(FDomainError::TargetError(err)) = err else {
            panic!("Wrong error type!")
        };

        assert_eq!(fidl::Status::PEER_CLOSED.into_raw(), err);

        for pair in msgs.chunks(2) {
            let a = &pair[0];
            let b = &pair[1];
            assert_eq!(test_str_a, a.bytes.as_slice());
            assert_eq!(test_str_b, b.bytes.as_slice());
            assert!(a.handles.is_empty());
            assert!(b.handles.is_empty());
        }
    };

    futures::future::join(read_side, write_side).await;
}

#[fuchsia::test]
async fn bad_tx() {
    let (client, fault_injector, fut) = TestFDomain::new_client_and_fut();
    let task = fuchsia_async::Task::spawn(fut);

    let (a, b) = client.create_channel();
    let test_str_a = b"Feral Cats Move In Mysterious Ways";
    a.fdomain_write(test_str_a, Vec::new()).await.unwrap();
    fault_injector.send_garbage(b"*splot*");
    let err = b.recv_msg().await.unwrap_err();
    let err2 = b.recv_msg().await.unwrap_err();
    assert_eq!(err.to_string(), err2.to_string());
    // Make sure the task exits once the transport dies.
    task.await;
}

#[fuchsia::test]
async fn channel_read_stream_read() {
    let (client, _) = TestFDomain::new_client();

    let (a, b) = client.create_channel();

    let read_1 = b.recv_msg();
    let read_2 = b.recv_msg();

    let test_str_a = b"Feral Cats Move In Mysterious Ways";
    let test_str_b = b"Almost all of our feelings were programmed in to us.";
    let test_str_c = b"Joyous Throbbing! Jubilant Pulsing!";
    a.fdomain_write(test_str_a, Vec::new()).await.unwrap();
    a.fdomain_write(test_str_b, Vec::new()).await.unwrap();
    a.fdomain_write(test_str_c, Vec::new()).await.unwrap();
    a.fdomain_write(test_str_b, Vec::new()).await.unwrap();

    let (mut stream, writer) = b.stream().unwrap();
    let read_1 = read_1.await.unwrap();
    let read_2 = read_2.await.unwrap();

    assert_eq!(test_str_a, read_1.bytes.as_slice());
    assert_eq!(test_str_b, read_2.bytes.as_slice());

    let stream_read = stream.next().await.unwrap().unwrap();
    assert_eq!(test_str_c, stream_read.bytes.as_slice());

    let b = stream.rejoin(writer);
    let read_3 = b.recv_msg().await.unwrap();
    assert_eq!(test_str_b, read_3.bytes.as_slice());
}

#[fuchsia::test]
async fn channel_too_big() {
    let (client, _) = TestFDomain::new_client();

    let (a, b) = client.create_channel();

    // Test with fdomain_write
    let err = a
        .fdomain_write(&[0xABu8; zx_types::ZX_CHANNEL_MAX_MSG_BYTES as usize + 1], vec![])
        .await
        .unwrap_err();

    let Error::FDomain(FDomainError::TargetError(i)) = err else { panic!() };

    assert_eq!(fidl::Status::OUT_OF_RANGE.into_raw(), i);

    let mut too_many_handles =
        Vec::with_capacity(zx_types::ZX_CHANNEL_MAX_MSG_HANDLES as usize + 1);
    for _ in 0..(zx_types::ZX_CHANNEL_MAX_MSG_HANDLES + 1) {
        too_many_handles.push(client.create_event().into_handle());
    }

    let err = a.fdomain_write(b"", too_many_handles).await.unwrap_err();

    let Error::FDomain(FDomainError::TargetError(i)) = err else { panic!() };

    assert_eq!(fidl::Status::OUT_OF_RANGE.into_raw(), i);

    // Test with zircon-like write
    let err =
        a.write(&[0xABu8; zx_types::ZX_CHANNEL_MAX_MSG_BYTES as usize + 1], vec![]).unwrap_err();

    let Error::FDomain(FDomainError::TargetError(i)) = err else { panic!() };

    assert_eq!(fidl::Status::OUT_OF_RANGE.into_raw(), i);

    let mut too_many_handles =
        Vec::with_capacity(zx_types::ZX_CHANNEL_MAX_MSG_HANDLES as usize + 1);
    for _ in 0..(zx_types::ZX_CHANNEL_MAX_MSG_HANDLES + 1) {
        too_many_handles.push(client.create_event().into_handle());
    }

    let err = a.write(b"", too_many_handles).unwrap_err();

    let Error::FDomain(FDomainError::TargetError(i)) = err else { panic!() };

    assert_eq!(fidl::Status::OUT_OF_RANGE.into_raw(), i);

    // Test with fdomain_write_etc
    let err = a
        .fdomain_write_etc(&[0xABu8; zx_types::ZX_CHANNEL_MAX_MSG_BYTES as usize + 1], vec![])
        .await
        .unwrap_err();

    let Error::FDomain(FDomainError::TargetError(i)) = err else { panic!() };

    assert_eq!(fidl::Status::OUT_OF_RANGE.into_raw(), i);

    let mut too_many_handles =
        Vec::with_capacity(zx_types::ZX_CHANNEL_MAX_MSG_HANDLES as usize + 1);
    for _ in 0..(zx_types::ZX_CHANNEL_MAX_MSG_HANDLES + 1) {
        too_many_handles
            .push(HandleOp::Move(client.create_event().into_handle(), fidl::Rights::SAME_RIGHTS));
    }

    let err = a.fdomain_write_etc(b"", too_many_handles).await.unwrap_err();

    let Error::FDomain(FDomainError::TargetError(i)) = err else { panic!() };

    assert_eq!(fidl::Status::OUT_OF_RANGE.into_raw(), i);

    // Make sure channel still functions
    const TEST_STR_1: &[u8] = b"Feral Cats Move In Mysterious Ways";

    a.fdomain_write(TEST_STR_1, vec![]).await.unwrap();

    let msg = b.recv_msg().await.unwrap();

    assert_eq!(TEST_STR_1, msg.bytes.as_slice());
}

#[fuchsia::test]
async fn client_drop_socket_read() {
    let (client, _fault_injector) = TestFDomain::new_client();

    let (_a, b) = client.create_stream_socket();

    let (notify_slept, has_slept) = futures::channel::oneshot::channel();
    let mut notify_slept = Some(notify_slept);

    let weak_client = Arc::downgrade(&client);
    let task = fuchsia_async::Task::spawn(async move {
        let mut buf = [0u8; 2];
        let mut fut = b.fdomain_read(&mut buf);
        futures::future::poll_fn(move |cx| {
            let res = fut.poll_unpin(cx);

            if res.is_pending() {
                if let Some(notify_slept) = notify_slept.take() {
                    notify_slept.send(()).unwrap();
                }
            } else {
                assert!(weak_client.upgrade().is_none());
            }

            res
        })
        .await
    });

    let _: () = has_slept.await.unwrap();

    std::mem::drop(client);
    let Err(Error::Transport(None)) = task.await else {
        panic!("Wrong error type!");
    };
}

#[fuchsia::test]
async fn client_drop_channel_read() {
    let (client, _fault_injector) = TestFDomain::new_client();

    let (_a, b) = client.create_channel();

    let (notify_slept, has_slept) = futures::channel::oneshot::channel();
    let mut notify_slept = Some(notify_slept);

    let weak_client = Arc::downgrade(&client);
    let task = fuchsia_async::Task::spawn(async move {
        let mut fut = b.recv_msg();
        futures::future::poll_fn(move |cx| {
            let res = fut.poll_unpin(cx);

            if res.is_pending() {
                if let Some(notify_slept) = notify_slept.take() {
                    notify_slept.send(()).unwrap();
                }
            } else {
                assert!(weak_client.upgrade().is_none());
            }

            res
        })
        .await
    });

    let _: () = has_slept.await.unwrap();

    std::mem::drop(client);
    let Err(Error::Transport(None)) = task.await else {
        panic!("Wrong error type!");
    };
}

#[fuchsia::test]
async fn client_drop_signals() {
    let (client, _fault_injector) = TestFDomain::new_client();

    let (a, b) = client.create_channel();

    let (notify_slept, has_slept) = futures::channel::oneshot::channel();
    let mut notify_slept = Some(notify_slept);

    let weak_client = Arc::downgrade(&client);
    let task = fuchsia_async::Task::spawn(async move {
        let mut fut = OnFDomainSignals::new(&b.as_handle_ref(), fidl::Signals::HANDLE_CLOSED);
        futures::future::poll_fn(move |cx| {
            let res = fut.poll_unpin(cx);

            if res.is_pending() {
                if let Some(notify_slept) = notify_slept.take() {
                    notify_slept.send(()).unwrap();
                }
            } else {
                assert!(weak_client.upgrade().is_none());
            }

            res
        })
        .await
    });

    let _: () = has_slept.await.unwrap();

    std::mem::drop(client);
    let Err(Error::Transport(None)) = task.await else {
        panic!("Wrong error type!");
    };

    assert!(a.as_handle_ref().is_invalid());
}

#[fuchsia::test]
async fn waker_reentrancy_test() {
    #[derive(Clone)]
    struct WakerData {
        real_waker: Waker,
        sock: Arc<Socket>,
    }

    unsafe fn clone(data: *const ()) -> RawWaker {
        let data = unsafe {
            Box::into_raw(Box::new((*data.cast::<WakerData>()).clone())).cast_const().cast::<()>()
        };
        RawWaker::new(data, VTABLE)
    }

    unsafe fn wake(data: *const ()) {
        let data = unsafe { Box::from_raw(data.cast_mut().cast::<WakerData>()) };
        let data = *data;
        let _ = data.sock.fdomain_write_all(b"Woken");
        data.real_waker.wake();
    }

    unsafe fn wake_by_ref(data: *const ()) {
        let data = unsafe { &*data.cast_mut().cast::<WakerData>() };
        let _ = data.sock.fdomain_write_all(b"Woken");
        data.real_waker.wake_by_ref();
    }

    unsafe fn drop(data: *const ()) {
        unsafe {
            let _ = Box::from_raw(data.cast_mut().cast::<WakerData>());
        }
    }

    static VTABLE: &RawWakerVTable = &RawWakerVTable::new(clone, wake, wake_by_ref, drop);

    let (client, _fault_injector) = TestFDomain::new_client();
    let (a, b) = client.create_stream_socket();
    let (notif_waker, mut notification) = client.create_stream_socket();
    let notif_waker = Arc::new(notif_waker);
    let mut buf = [0u8; b"Message".len()];
    let mut read = 0;

    let task = fuchsia_async::Task::spawn(futures::future::poll_fn(move |cx| {
        let waker_data =
            Box::new(WakerData { real_waker: cx.waker().clone(), sock: Arc::clone(&notif_waker) });

        let waker =
            unsafe { Waker::new(Box::into_raw(waker_data).cast_const().cast::<()>(), VTABLE) };
        let mut cx = Context::from_waker(&waker);
        while read < buf.len() {
            read += ready!(a.poll_socket(&mut cx, &mut buf[read..]))?;
        }
        Poll::Ready(Result::<(), crate::Error>::Ok(()))
    }));

    b.fdomain_write_all(b"Message").await.unwrap();
    task.await.unwrap();
    let mut buf = [0u8; b"Woken".len()];
    notification.read_exact(&mut buf).await.unwrap();
    assert_eq!(b"Woken", &buf);
}

#[fuchsia::test]
async fn failed_transport() {
    let (client, fault_injector) = TestFDomain::new_client();
    let (a, b) = client.create_channel();
    const TEST_STR_A: &[u8] = b"Feral Cats Move In Mysterious Ways";

    fault_injector.inject(TestError("Connection failed".to_owned()));

    let err = a.fdomain_write(TEST_STR_A, Vec::new()).await.unwrap_err();
    let Error::Transport(Some(err)) = err else { panic!("Wrong error type!") };

    let TestError(err) = err.get_ref().unwrap().downcast_ref().unwrap();
    assert_eq!("Connection failed", err);

    let err = b.close().await.unwrap_err();
    let Error::Transport(Some(err)) = err else { panic!("Wrong error type!") };

    let TestError(err) = err.get_ref().unwrap().downcast_ref().unwrap();
    assert_eq!("Connection failed", err);
}

#[fuchsia::test]
async fn handle_id_collision_avoidance() {
    let (client, _) = TestFDomain::new_client();

    let mut handles = Vec::new();
    let mut ids = std::collections::HashSet::new();

    // Allocate many handles and ensure all generated IDs are distinct, non-zero,
    // and have bit 31 clear.
    for _ in 0..1000 {
        let (a, b) = client.create_channel();
        let id_a = a.as_handle_ref().u32_id();
        let id_b = b.as_handle_ref().u32_id();

        assert_ne!(id_a, 0);
        assert_ne!(id_b, 0);
        assert_eq!(id_a & (1 << 31), 0);
        assert_eq!(id_b & (1 << 31), 0);

        assert!(ids.insert(id_a), "Duplicate handle ID {id_a} allocated!");
        assert!(ids.insert(id_b), "Duplicate handle ID {id_b} allocated!");

        handles.push((a, b));
    }
}

#[fuchsia::test]
async fn handle_lifecycle_tracking() {
    let (client, _) = TestFDomain::new_client();

    let initial_count = client.0.lock().handles.len();
    assert_eq!(initial_count, 0);

    // Create channel -> 2 handles
    let (a, b) = client.create_channel();
    assert_eq!(client.0.lock().handles.len(), 2);
    assert!(client.0.lock().handles.contains(&a.as_handle_ref().proto()));
    assert!(client.0.lock().handles.contains(&b.as_handle_ref().proto()));

    // Create event -> 1 more handle
    let event = client.create_event();
    assert_eq!(client.0.lock().handles.len(), 3);
    assert!(client.0.lock().handles.contains(&event.as_handle_ref().proto()));

    // Duplicate event -> 1 more handle
    let duplicated = event.duplicate_handle(fidl::Rights::SAME_RIGHTS).await.unwrap();
    assert_eq!(client.0.lock().handles.len(), 4);
    assert!(client.0.lock().handles.contains(&duplicated.as_handle_ref().proto()));

    // Explicit close event -> removes 1 handle
    event.close().await.unwrap();
    assert_eq!(client.0.lock().handles.len(), 3);

    // Transfer duplicated handle through channel -> removes 1 handle
    a.fdomain_write(b"transfer", vec![duplicated.into_handle()]).await.unwrap();
    assert_eq!(client.0.lock().handles.len(), 2);

    // Explicit close b -> removes 1 handle
    b.close().await.unwrap();
    assert_eq!(client.0.lock().handles.len(), 1);

    // Explicit close a -> removes last handle
    a.close().await.unwrap();
    assert_eq!(client.0.lock().handles.len(), 0);
}
#[fuchsia::test]
async fn vmo_basic() {
    let (client, _) = TestFDomain::new_client();
    let vmo = client.create_vmo(VmoOptions::RESIZABLE, 4096);

    assert_eq!(vmo.get_size().await.unwrap(), 4096);

    vmo.write(b"Hello VMO", 0).await.unwrap();

    let data = vmo.read(0, 9).await.unwrap();
    assert_eq!(data, b"Hello VMO");

    let mut buf = [0u8; 9];
    vmo.read_slice(&mut buf, 0).await.unwrap();
    assert_eq!(&buf, b"Hello VMO");

    vmo.set_size(8192).await.unwrap();
    assert_eq!(vmo.get_size().await.unwrap(), 8192);

    assert_eq!(vmo.get_stream_size().await.unwrap(), 8192);
    vmo.set_stream_size(128).await.unwrap();
    assert_eq!(vmo.get_stream_size().await.unwrap(), 128);

    // Non-resizable VMO
    let non_resizable_vmo = client.create_vmo(VmoOptions::empty(), 4096);
    assert_eq!(non_resizable_vmo.get_size().await.unwrap(), 4096);
    assert!(non_resizable_vmo.set_size(8192).await.is_err());
}

#[fuchsia::test]
async fn vmo_over_channel() {
    let (client, _) = TestFDomain::new_client();
    let (a, b) = client.create_channel();
    let vmo = client.create_vmo(VmoOptions::RESIZABLE, 512);
    vmo.write(b"Data in VMO", 0).await.unwrap();

    a.fdomain_write(b"msg", vec![vmo.into()]).await.unwrap();

    let mut msg = b.recv_msg().await.unwrap();
    assert_eq!(msg.bytes.as_slice(), b"msg");
    assert_eq!(msg.handles.len(), 1);

    let handle_info = msg.handles.pop().unwrap();
    let AnyHandle::Vmo(vmo_received) = handle_info.handle else {
        panic!("Expected Vmo handle");
    };

    let data = vmo_received.read(0, 11).await.unwrap();
    assert_eq!(data, b"Data in VMO");
}

#[fuchsia::test]
async fn test_signals() {
    let (client, _) = TestFDomain::new_client();
    let event = client.create_event();

    // Signal USER_0: clear NONE, set USER_0
    event.as_handle_ref().signal(fidl::Signals::NONE, fidl::Signals::USER_0).await.unwrap();

    let signals =
        OnFDomainSignals::new(&event.as_handle_ref(), fidl::Signals::USER_0).await.unwrap();
    assert_eq!(signals, fidl::Signals::USER_0);

    // Clear USER_0 and set USER_1 using signal_handle
    event.signal_handle(fidl::Signals::USER_0, fidl::Signals::USER_1).await.unwrap();

    let signals =
        OnFDomainSignals::new(&event.as_handle_ref(), fidl::Signals::USER_1).await.unwrap();
    assert_eq!(signals, fidl::Signals::USER_1);

    // Clear USER_1 and set no signals using signal
    event.as_handle_ref().signal(fidl::Signals::USER_1, fidl::Signals::NONE).await.unwrap();

    let (a, b) = client.create_event_pair();

    // Signal peer from a: clear NONE, set USER_2 on b
    a.signal_peer(fidl::Signals::NONE, fidl::Signals::USER_2).await.unwrap();

    let signals = OnFDomainSignals::new(&b.as_handle_ref(), fidl::Signals::USER_2).await.unwrap();
    assert_eq!(signals, fidl::Signals::USER_2);

    // Signal peer from b: clear NONE, set USER_3 on a
    b.signal_peer(fidl::Signals::NONE, fidl::Signals::USER_3).await.unwrap();

    let signals = OnFDomainSignals::new(&a.as_handle_ref(), fidl::Signals::USER_3).await.unwrap();
    assert_eq!(signals, fidl::Signals::USER_3);
}

#[fuchsia::test]
async fn client_loop_drop_sets_transport_error() {
    let (client, _fault_injector, fut) = TestFDomain::new_client_and_fut();
    let (a, b) = client.create_channel();
    let (s1, s2) = client.create_stream_socket();

    assert!(client.transport_status().is_ok());

    std::mem::drop(fut);

    assert!(matches!(client.transport_status(), Err(Error::Transport(None))));

    assert!(matches!(client.namespace().await, Err(Error::Transport(None))));
    assert!(matches!(a.fdomain_write(b"test", vec![]).await, Err(Error::Transport(None))));
    assert!(matches!(b.recv_msg().await, Err(Error::Transport(None))));
    assert!(matches!(s1.fdomain_write_all(b"test").await, Err(Error::Transport(None))));
    let mut buf = [0u8; 4];
    assert!(matches!(s2.fdomain_read(&mut buf).await, Err(Error::Transport(None))));
    assert!(matches!(b.stream(), Err(Error::Transport(None))));
    assert!(matches!(s2.stream(), Err(Error::Transport(None))));
}

#[fuchsia::test]
async fn socket_read_stream_stop() {
    let (client, _) = TestFDomain::new_client();
    let (a, b) = client.create_stream_socket();
    let b_hid = b.0.proto();

    let (mut stream, writer) = b.stream().unwrap();
    assert!(client.0.lock().socket_read_states.get(&b_hid).unwrap().is_streaming);

    a.fdomain_write_all(b"hello").await.unwrap();
    let mut buf = [0u8; 5];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"hello");

    let b = stream.rejoin(writer);
    assert!(!client.0.lock().socket_read_states.get(&b_hid).unwrap().is_streaming);

    a.fdomain_write_all(b"world").await.unwrap();
    let mut buf2 = [0u8; 5];
    let n = b.fdomain_read(&mut buf2).await.unwrap();
    assert_eq!(&buf2[..n], b"world");
}

#[fuchsia::test]
async fn socket_read_request_pending_deduplication() {
    let (client, _) = TestFDomain::new_client();
    let (a, b) = client.create_stream_socket();
    let b_hid = b.0.proto();

    let mut buf1 = [0u8; 5];
    let mut buf2 = [0u8; 5];
    let mut fut1 = b.fdomain_read(&mut buf1);
    let mut fut2 = b.fdomain_read(&mut buf2);

    let mut cx = Context::from_waker(Waker::noop());
    assert!(fut1.poll_unpin(&mut cx).is_pending());
    assert!(client.0.lock().socket_read_states.get(&b_hid).unwrap().read_request_pending);
    let tx_count = client.0.lock().transactions.len();

    // Subsequent polls while pending must not issue duplicate READ_SOCKET requests.
    assert!(fut1.poll_unpin(&mut cx).is_pending());
    assert!(fut2.poll_unpin(&mut cx).is_pending());
    assert!(fut2.poll_unpin(&mut cx).is_pending());
    assert_eq!(client.0.lock().transactions.len(), tx_count);

    std::mem::drop(fut2);

    a.fdomain_write_all(b"hello").await.unwrap();
    let n = fut1.await.unwrap();
    assert_eq!(&buf1[..n], b"hello");
    assert!(!client.0.lock().socket_read_states.get(&b_hid).unwrap().read_request_pending);

    // Ensure no orphaned READ_SOCKET request remains on the server when switching to streaming.
    let (mut stream, _writer) = b.stream().unwrap();
    a.fdomain_write_all(b"world").await.unwrap();
    let mut stream_buf = [0u8; 5];
    stream.read_exact(&mut stream_buf).await.unwrap();
    assert_eq!(&stream_buf, b"world");
}

#[fuchsia::test]
async fn channel_read_request_pending_deduplication() {
    let (client, _) = TestFDomain::new_client();
    let (a, b) = client.create_channel();
    let b_hid = b.0.proto();

    let mut fut1 = b.recv_msg();
    let mut fut2 = b.recv_msg();

    let mut cx = Context::from_waker(Waker::noop());
    assert!(fut1.poll_unpin(&mut cx).is_pending());
    assert!(client.0.lock().channel_read_states.get(&b_hid).unwrap().read_request_pending);
    let tx_count = client.0.lock().transactions.len();

    // Subsequent polls while pending must not issue duplicate READ_CHANNEL requests.
    assert!(fut1.poll_unpin(&mut cx).is_pending());
    assert!(fut2.poll_unpin(&mut cx).is_pending());
    assert!(fut2.poll_unpin(&mut cx).is_pending());
    assert_eq!(client.0.lock().transactions.len(), tx_count);

    std::mem::drop(fut2);

    a.fdomain_write(b"hello", vec![]).await.unwrap();
    let msg = fut1.await.unwrap();
    assert_eq!(msg.bytes.as_slice(), b"hello");
    assert!(!client.0.lock().channel_read_states.get(&b_hid).unwrap().read_request_pending);

    // Ensure no orphaned READ_CHANNEL request remains on the server when switching to streaming.
    let (mut stream, _writer) = b.stream().unwrap();
    a.fdomain_write(b"world", vec![]).await.unwrap();
    let stream_msg = stream.next().await.unwrap().unwrap();
    assert_eq!(stream_msg.bytes.as_slice(), b"world");
}
