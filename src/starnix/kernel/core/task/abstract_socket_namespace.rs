// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::CurrentTask;
use crate::vfs::FsString;
use crate::vfs::socket::{Socket, SocketAddress, SocketHandle};
use fuchsia_rcu::RcuDroppable;
use starnix_rcu::RcuReadScope;
use starnix_rcu::rcu_hash_map::{Entry, RcuHashMap};
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};
use std::sync::{Arc, Weak};

/// A registry of abstract sockets.
///
/// AF_UNIX sockets can be bound either to nodes in the file system or to
/// abstract addresses that are independent of the file system. This object
/// holds the bindings to abstract addresses.
///
/// See "abstract" in https://man7.org/linux/man-pages/man7/unix.7.html
pub struct AbstractSocketNamespace<K>
where
    K: std::cmp::Eq + std::hash::Hash + Clone + RcuDroppable + Sync,
{
    table: RcuHashMap<K, Weak<Socket>>,
    address_maker: Box<dyn Fn(K) -> SocketAddress + Send + Sync>,
}

pub type AbstractUnixSocketNamespace = AbstractSocketNamespace<FsString>;
pub type AbstractVsockSocketNamespace = AbstractSocketNamespace<u32>;

impl<K> AbstractSocketNamespace<K>
where
    K: std::cmp::Eq + std::hash::Hash + Clone + RcuDroppable + Sync,
{
    pub fn new(
        address_maker: Box<dyn Fn(K) -> SocketAddress + Send + Sync>,
    ) -> Arc<AbstractSocketNamespace<K>> {
        Arc::new(AbstractSocketNamespace::<K> { table: RcuHashMap::default(), address_maker })
    }

    pub fn bind(
        &self,
        current_task: &CurrentTask,
        address: K,
        socket: &SocketHandle,
    ) -> Result<(), Errno> {
        let mut table = self.table.lock();
        match table.entry(address.clone()) {
            Entry::Vacant(entry) => {
                socket.bind(current_task, (self.address_maker)(address))?;
                entry.insert(Arc::downgrade(socket));
            }
            Entry::Occupied(mut entry) => {
                let occupant = entry.get().upgrade();
                if occupant.is_some() {
                    return error!(EADDRINUSE);
                }
                socket.bind(current_task, (self.address_maker)(address))?;
                entry.insert(Arc::downgrade(socket));
            }
        }
        Ok(())
    }

    pub fn lookup<Q: ?Sized>(&self, address: &Q) -> Result<SocketHandle, Errno>
    where
        K: std::borrow::Borrow<Q>,
        Q: std::hash::Hash + Eq,
    {
        let scope = RcuReadScope::new();
        self.table
            .get(&scope, address)
            .and_then(|weak| weak.upgrade())
            .ok_or_else(|| errno!(ECONNREFUSED))
    }
}
