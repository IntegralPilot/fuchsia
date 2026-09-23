// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::app::strategies::base::AppStrategy;
use crate::app::{InternalSender, MessageInternal};
use crate::view::strategies::base::{FlatlandParams, ViewStrategyParams, ViewStrategyPtr};
use crate::view::strategies::flatland::{FlatlandViewStrategy, FLATLAND_DEBUG_NAME};
use crate::view::ViewKey;
use anyhow::{bail, Context as _, Error};
use async_trait::async_trait;
use fidl_fuchsia_ui_app::{CreateView2Args, ViewProviderRequest, ViewProviderRequestStream};
use fuchsia_async::{self as fasync};
use fuchsia_component::server::{ServiceFs, ServiceObjLocal};
use fuchsia_scenic::flatland::ViewCreationTokenPair;
use futures::channel::mpsc::UnboundedSender;
use futures::{TryFutureExt, TryStreamExt};

pub(crate) struct FlatlandAppStrategy;

#[async_trait(?Send)]
impl AppStrategy for FlatlandAppStrategy {
    async fn create_view_strategy(
        &mut self,
        key: ViewKey,
        app_sender: UnboundedSender<MessageInternal>,
        strategy_params: ViewStrategyParams,
    ) -> Result<ViewStrategyPtr, Error> {
        match strategy_params {
            ViewStrategyParams::Flatland(flatland_params) => {
                Ok(FlatlandViewStrategy::new(key, flatland_params, app_sender.clone()).await?)
            }
            _ => bail!(
                "Incorrect ViewStrategyParams passed to create_view_strategy for Scenic (Flatland)"
            ),
        }
    }

    fn create_view_strategy_params_for_additional_view(
        &mut self,
        _view_key: ViewKey,
    ) -> ViewStrategyParams {
        todo!("Additional views not yet supported on Scenic (Flatland)");
    }

    fn create_view_for_testing(
        &self,
        app_sender: &UnboundedSender<MessageInternal>,
    ) -> Result<(), Error> {
        let tokens = ViewCreationTokenPair::new().context("ViewCreationTokenPair::new")?;
        app_sender
            .unbounded_send(MessageInternal::CreateView(ViewStrategyParams::Flatland(
                FlatlandParams {
                    args: CreateView2Args {
                        view_creation_token: Some(tokens.view_creation_token),
                        ..Default::default()
                    },
                    debug_name: Some("Test View".to_string()),
                },
            )))
            .expect("send");
        Ok(())
    }

    fn supports_scenic(&self) -> bool {
        return true;
    }

    fn start_services<'a, 'b>(
        &self,
        app_sender: UnboundedSender<MessageInternal>,
        fs: &'a mut ServiceFs<ServiceObjLocal<'b, ()>>,
    ) -> Result<(), Error> {
        let mut public = fs.dir("svc");

        let sender = app_sender;
        let f = move |stream: ViewProviderRequestStream| {
            let sender = sender.clone();
            fasync::Task::local(
                stream
                    .try_for_each(move |req| {
                        #[allow(unreachable_patterns)]
                        match req {
                            ViewProviderRequest::CreateView2 { args, .. } => {
                                sender
                                    .unbounded_send(MessageInternal::CreateView(
                                        ViewStrategyParams::Flatland(FlatlandParams {
                                            args,
                                            debug_name: Some(FLATLAND_DEBUG_NAME.to_string()),
                                        }),
                                    ))
                                    .expect("unbounded_send");
                            }

                            _ => {}
                        };
                        futures::future::ready(Ok(()))
                    })
                    .unwrap_or_else(|e| eprintln!("error running ViewProvider server: {:?}", e)),
            )
            .detach()
        };
        public.add_fidl_service(f);

        Ok(())
    }

    async fn post_setup(&mut self, _internal_sender: &InternalSender) -> Result<(), Error> {
        Ok(())
    }
}
