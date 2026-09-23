// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use sapphire_async::executor::BoundedExecutor;
use sapphire_async::rpc::{RpcCfg, RpcChannel};
use sapphire_async::testing::TestExecutor;
use sapphire_collections::storage::ArrayStorage;
use sapphire_rpc_macro::rpc;
use sapphire_sync::mutex::raw::SingleThreadMutex;

struct TestCfg;
impl RpcCfg for TestCfg {
    type Mtx = SingleThreadMutex;
    type Chan = ArrayStorage<10>;
}

struct MockService {
    add_count: usize,
    greet_count: usize,
}

impl MockService {
    fn new() -> Self {
        Self { add_count: 0, greet_count: 0 }
    }
}

#[rpc]
impl MockService {
    async fn add(&mut self, x: i32, y: i32) -> i32 {
        self.add_count += 1;
        x + y
    }

    async fn greet(&mut self, name: String) -> String {
        self.greet_count += 1;
        format!("Hello, {}", name)
    }

    // it can handle sync calls too
    fn ping(&self) {
        // unit return type, sync fn
    }

    pub async fn echo(&self, val: u32) -> u32 {
        val
    }
}

#[test]
fn test_rpc_macro_basic() {
    let mut channel = RpcChannel::<MockServiceRpc, TestCfg>::new();
    let (client_handle, server_handle) = channel.split();
    let client = MockServiceClient::new(client_handle);

    let mut server = MockService::new();

    BoundedExecutor::new(TestExecutor::new(), |s| {
        // Spawn server loop
        s.spawn(async {
            while let Ok((req, responder)) = server_handle.recv().await {
                server.route_request(req, responder).await;
            }
        });

        s.block_on(async {
            assert_eq!(client.add(10, 20).await.unwrap(), 30);
            assert_eq!(client.greet("Fuchsia".to_string()).await.unwrap(), "Hello, Fuchsia");
            client.ping().await.unwrap();
            assert_eq!(client.echo(42).await.unwrap(), 42);
        });
    });
}

struct ImmutableService;

#[rpc]
impl ImmutableService {
    fn new_instance() -> Self {
        Self
    }

    async fn get_data(&self, id: u32) -> Result<String, i32> {
        if id == 0 { Err(-1) } else { Ok(format!("data_{}", id)) }
    }
}

#[test]
fn test_rpc_macro_immutable_and_assoc_fn() {
    let mut channel = RpcChannel::<ImmutableServiceRpc, TestCfg>::new();
    let (client_handle, server_handle) = channel.split();
    let client = ImmutableServiceClient::new(client_handle);

    // Verify associated function works
    let server = ImmutableService::new_instance();

    BoundedExecutor::new(TestExecutor::new(), |s| {
        // Since the service is fully immutable, we can have multiple handlers concurrently.
        for _ in 0..4 {
            s.spawn(async {
                while let Ok((req, responder)) = server_handle.recv().await {
                    // Notice: server is immutable (&server), not &mut server
                    server.route_request(req, responder).await;
                }
            });
        }

        s.block_on(async {
            assert_eq!(client.get_data(10).await.unwrap(), Ok("data_10".to_string()));
            assert_eq!(client.get_data(0).await.unwrap(), Err(-1));
        });
    });
}

struct GenericService<T> {
    item: T,
}

#[rpc]
impl<T: Clone + 'static> GenericService<T> {
    async fn get_item(&self) -> T {
        self.item.clone()
    }

    async fn set_item(&mut self, new_item: T) -> T {
        core::mem::replace(&mut self.item, new_item)
    }
}

#[test]
fn test_rpc_macro_generics() {
    let mut channel = RpcChannel::<GenericServiceRpc<i32>, TestCfg>::new();
    let (client_handle, server_handle) = channel.split();
    let client = GenericServiceClient::new(client_handle);

    let mut server = GenericService { item: 100 };

    BoundedExecutor::new(TestExecutor::new(), |s| {
        s.spawn(async {
            while let Ok((req, responder)) = server_handle.recv().await {
                server.route_request(req, responder).await;
            }
        });

        s.block_on(async {
            assert_eq!(client.get_item().await.unwrap(), 100);
            assert_eq!(client.set_item(200).await.unwrap(), 100);
            assert_eq!(client.get_item().await.unwrap(), 200);
        });
    });
}

struct LifetimeConstService<'a, const N: usize> {
    prefix: &'a str,
}

#[rpc]
impl<'a, const N: usize> LifetimeConstService<'a, N> {
    async fn format_data(&self, suffix: &'a str) -> String {
        format!("{}_{}_{}", self.prefix, suffix, N)
    }

    async fn get_size(&self) -> usize {
        N
    }
}

#[test]
fn test_rpc_macro_lifetimes_and_const_generics() {
    let prefix = "test_prefix";
    let mut channel = RpcChannel::<LifetimeConstServiceRpc<'_, 8>, TestCfg>::new();
    let (client_handle, server_handle) = channel.split();
    let client = LifetimeConstServiceClient::new(client_handle);

    let server = LifetimeConstService::<8> { prefix };

    BoundedExecutor::new(TestExecutor::new(), |s| {
        s.spawn(async {
            while let Ok((req, responder)) = server_handle.recv().await {
                server.route_request(req, responder).await;
            }
        });

        s.block_on(async {
            assert_eq!(client.format_data("suffix").await.unwrap(), "test_prefix_suffix_8");
            assert_eq!(client.get_size().await.unwrap(), 8);
        });
    });
}

struct ComplexService<'a, T, const N: usize> {
    data: &'a [T; N],
}

#[rpc]
impl<'a, T, const N: usize> ComplexService<'a, T, N>
where
    T: Clone + 'static,
{
    async fn get_at(&self, idx: usize) -> Option<T> {
        self.data.get(idx).cloned()
    }
}

#[test]
fn test_rpc_macro_complex_generics_and_where_clause() {
    let array = [10, 20, 30, 40];
    let mut channel = RpcChannel::<ComplexServiceRpc<'_, i32, 4>, TestCfg>::new();
    let (client_handle, server_handle) = channel.split();
    let client = ComplexServiceClient::new(client_handle);
    let client2 = client.clone();

    let server = ComplexService { data: &array };

    BoundedExecutor::new(TestExecutor::new(), |s| {
        s.spawn(async {
            while let Ok((req, responder)) = server_handle.recv().await {
                server.route_request(req, responder).await;
            }
        });

        s.block_on(async {
            assert_eq!(client.get_at(0).await.unwrap(), Some(10));
            assert_eq!(client2.get_at(2).await.unwrap(), Some(30));
            assert_eq!(client.get_at(10).await.unwrap(), None);
        });
    });
}

struct PureConstService<const N: usize>;

#[rpc]
impl<const N: usize> PureConstService<N> {
    async fn get_capacity(&self) -> usize {
        N
    }
}

#[test]
fn test_rpc_macro_pure_const_generic() {
    let mut channel = RpcChannel::<PureConstServiceRpc<64>, TestCfg>::new();
    let (client_handle, server_handle) = channel.split();
    let client = PureConstServiceClient::new(client_handle);

    let server = PureConstService::<64>;

    BoundedExecutor::new(TestExecutor::new(), |s| {
        s.spawn(async {
            while let Ok((req, responder)) = server_handle.recv().await {
                server.route_request(req, responder).await;
            }
        });

        s.block_on(async {
            assert_eq!(client.get_capacity().await.unwrap(), 64);
        });
    });
}
