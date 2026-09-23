// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Interface local route tables.

use std::sync::Arc;

use derivative::Derivative;
use fidl_fuchsia_net_routes_ext::FidlRouteIpExt;
use fidl_fuchsia_net_routes_ext::admin::FidlRouteAdminIpExt;
use fuchsia_async as fasync;
use futures::channel::mpsc;
use net_types::ip::{GenericOverIp, Ip, Ipv4, Ipv6};

use crate::bindings::Ctx;
use crate::bindings::routes::admin::{RouteTable, UserRouteSet};
use crate::bindings::routes::{self, RouteWorkItem, TableId, TableIdOverflowsError};
use crate::bindings::util::ScopeExt as _;

#[derive(Derivative, GenericOverIp)]
#[derivative(Debug)]
#[generic_over_ip(I, Ip)]
pub(crate) struct LocalRouteTable<I: Ip> {
    pub(crate) table_id: TableId<I>,
    #[derivative(Debug = "ignore")]
    route_work_sink: mpsc::UnboundedSender<RouteWorkItem<I>>,
    #[derivative(Debug = "ignore")]
    authorization_token: Arc<zx::Event>,
    #[derivative(Debug = "ignore")]
    scope: fasync::ScopeHandle,
}

impl<I: Ip> LocalRouteTable<I> {
    pub(crate) async fn new(
        ctx: &Ctx,
        if_name: &str,
        scope: fasync::ScopeHandle,
    ) -> Result<Self, TableIdOverflowsError> {
        let (table_id, route_work_sink, authorization_token) = ctx
            .bindings_ctx()
            .routes
            .add_table(Some(format!("{}-{:?}", if_name, I::VERSION)))
            .await?;
        Ok(Self { table_id, route_work_sink, authorization_token, scope })
    }
}

// Local route tables are reference counted: each user route set spawned from
// the table counts as a reference, so does the interface itself. The table
// is only destroyed when there are no more references to it.
impl<I: FidlRouteAdminIpExt + FidlRouteIpExt> RouteTable<I> for Arc<LocalRouteTable<I>> {
    fn id(&self) -> TableId<I> {
        self.table_id
    }

    fn token(&self) -> zx::Event {
        self.authorization_token
            .duplicate_handle(zx::Rights::TRANSFER | zx::Rights::DUPLICATE)
            .expect("failed to duplicate")
    }

    async fn remove(self) -> Result<(), routes::admin::TableRemoveError> {
        Err(routes::admin::TableRemoveError::InvalidOp)
    }

    fn detach(&mut self) {}

    fn detached(&self) -> bool {
        // Don't remove the table when stop serving the table.
        true
    }

    fn serve_user_route_set(
        &self,
        stream: <I as FidlRouteAdminIpExt>::RouteSetRequestStream,
        ctx: &Ctx,
    ) {
        routes::admin::spawn_user_route_set_and_keep_alive(
            &self.scope,
            stream,
            || UserRouteSet::new(self.table_id, self.route_work_sink.clone()),
            ctx,
            // Each open user route set will keep the local table open.
            Arc::clone(&self),
        );
    }
}

// This drop implementation only initiates the removal of the table but does
// not wait for the removal to complete. This is fine because this drop impl
// happens after the interface is removed and there are no outstanding user
// route set referencing the table; No one can send another route work down
// the work sink after this.
impl<I: Ip> Drop for LocalRouteTable<I> {
    fn drop(&mut self) {
        if let Err(err) = self.route_work_sink.unbounded_send(RouteWorkItem {
            change: routes::Change::RemoveTable(self.table_id),
            responder: None,
        }) {
            log::error!("ChangeRunner has stopped: {err:?}")
        }
    }
}

#[derive(Debug)]
pub(crate) struct LocalRouteTables {
    v4: Arc<LocalRouteTable<Ipv4>>,
    v6: Arc<LocalRouteTable<Ipv6>>,
}

impl LocalRouteTables {
    pub(crate) async fn new(ctx: &Ctx, if_name: &str) -> Result<Self, TableIdOverflowsError> {
        // Let the parent scope handle the cancellation.
        let handle = fasync::Scope::current().new_detached_child(format!("{if_name}-local-tables"));
        Ok(Self {
            v4: Arc::new(LocalRouteTable::<Ipv4>::new(ctx, if_name, handle.clone()).await?),
            v6: Arc::new(LocalRouteTable::<Ipv6>::new(ctx, if_name, handle).await?),
        })
    }

    pub(crate) fn get<I: Ip>(&self) -> &Arc<LocalRouteTable<I>> {
        I::map_ip_out(&self, |this| &this.v4, |this| &this.v6)
    }
}
