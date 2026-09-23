// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::offer_injection::OfferInjector;
use driver_manager_types::{
    NodeOffer, OfferTransport, StartRequest, StartRequestReceiver, StartedComponent,
};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_process as fprocess;
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceObjLocal};
use futures::TryStreamExt;
use futures::channel::mpsc;
use log::{error, warn};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const TOKEN_ID: u32 =
    fuchsia_runtime::HandleInfo::new(fuchsia_runtime::HandleType::User0, 0).as_raw();

#[derive(Clone)]
pub struct Runner {
    pub realm: fcomponent::RealmProxy,
    pub introspector: fcomponent::IntrospectorProxy,
    offer_injector: OfferInjector,
    pending_starts: Rc<RefCell<HashMap<String, StartRequest>>>,
    koid_to_moniker: Rc<RefCell<HashMap<zx::Koid, String>>>,
}

impl Runner {
    pub fn new(
        realm: fcomponent::RealmProxy,
        introspector: fcomponent::IntrospectorProxy,
        offer_injector: OfferInjector,
    ) -> Self {
        Self {
            realm,
            introspector,
            offer_injector,
            pending_starts: Rc::new(RefCell::new(HashMap::new())),
            koid_to_moniker: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    pub fn publish<'a>(&self, fs: &mut ServiceFs<ServiceObjLocal<'a, ()>>) {
        let runner = self.clone();
        fs.dir("svc").add_fidl_service(move |stream| {
            let runner = runner.clone();
            fasync::Task::local(async move {
                if let Err(e) = runner.serve(stream).await {
                    warn!("Failed to serve ComponentRunner: {}", e);
                }
            })
            .detach();
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_driver_component(
        &self,
        moniker: &str,
        url: &str,
        collection_name: &str,
        offers: &[NodeOffer],
        dictionary_ref: Option<fsandbox::DictionaryRef>,
        skip_injected_offers: bool,
        controller: fidl::endpoints::ServerEnd<fcomponent::ControllerMarker>,
    ) -> Result<(fprocess::HandleInfo, StartRequestReceiver), zx::Status> {
        let child_decl = fdecl::Child {
            name: Some(moniker.to_string()),
            url: Some(url.to_string()),
            startup: Some(fdecl::StartupMode::Lazy),
            ..Default::default()
        };

        // Filter out the dictionary offers as they are not supported through the component
        // dynamic offers.
        let mut dynamic_offers: Vec<fdecl::Offer> = offers
            .iter()
            .filter(|offer| !matches!(offer.transport, OfferTransport::Dictionary))
            .map(|offer| {
                let fdf_offer = offer.into();
                match fdf_offer {
                    fidl_fuchsia_driver_framework::Offer::ZirconTransport(offer) => offer,
                    fidl_fuchsia_driver_framework::Offer::DriverTransport(offer) => offer,
                    _ => panic!("Unknown offer type"),
                }
            })
            .collect();

        if !skip_injected_offers {
            let extra_offers_count = self.offer_injector.extra_offers_count();
            let current_len = dynamic_offers.len();
            dynamic_offers.resize_with(current_len + extra_offers_count, || {
                fdecl::Offer::Protocol(fdecl::OfferProtocol::default())
            });
            self.offer_injector.inject(&mut dynamic_offers, current_len);
        }

        let create_child_args = fcomponent::CreateChildArgs {
            dynamic_offers: Some(dynamic_offers),
            dictionary: dictionary_ref,
            controller: Some(controller),
            ..Default::default()
        };

        let collection_ref = fdecl::CollectionRef { name: collection_name.to_string() };

        self.realm
            .create_child(&collection_ref, &child_decl, create_child_args)
            .await
            .map_err(|e| {
                error!("Failed to create child {}: {}", moniker, e);
                zx::Status::INTERNAL
            })?
            .map_err(|e| {
                error!("Failed to create child {}: {:?}", moniker, e);
                zx::Status::INTERNAL
            })?;

        let (tx, rx) = mpsc::channel(1);
        self.pending_starts.borrow_mut().insert(moniker.to_string(), tx);
        let token = zx::Event::create();
        let koid = token.basic_info()?.koid;
        self.koid_to_moniker.borrow_mut().insert(koid, moniker.to_string());
        let handle_info = fprocess::HandleInfo { handle: token.into(), id: TOKEN_ID };

        Ok((handle_info, rx))
    }

    fn complete_request(&self, moniker: &str, result: Result<StartedComponent, zx::Status>) {
        if let Some(completer) = self.pending_starts.borrow_mut().get_mut(moniker) {
            let _: Result<(), _> = completer.try_send(result).map_err(|e| {
                error!("Failed to complete request for {}: {}", moniker, e);
                zx::Status::INTERNAL
            });
        }
    }

    async fn serve(
        &self,
        mut stream: frunner::ComponentRunnerRequestStream,
    ) -> Result<(), fidl::Error> {
        while let Some(request) = stream.try_next().await? {
            match request {
                frunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                    self.start(start_info, controller).await;
                }
                frunner::ComponentRunnerRequest::_UnknownMethod { .. } => {}
            }
        }
        Ok(())
    }

    async fn start(
        &self,
        start_info: frunner::ComponentStartInfo,
        controller: fidl::endpoints::ServerEnd<frunner::ComponentControllerMarker>,
    ) {
        let url = start_info.resolved_url.as_deref().unwrap_or("");

        let moniker = if let Some(handles) = start_info.numbered_handles.as_ref()
            && handles.len() == 1
            && handles[0].id == TOKEN_ID
        {
            let Ok(info) = handles[0].handle.basic_info() else {
                controller.close_with_epitaph(zx::Status::INVALID_ARGS).ok();
                return;
            };

            self.koid_to_moniker.borrow_mut().remove(&info.koid)
        } else {
            None
        };

        let moniker = if let Some(moniker) = moniker {
            moniker
        } else {
            // Fallback to Introspector
            let Some(component_instance) = start_info.component_instance.as_ref() else {
                error!("Failed to start driver '{}', no component instance", url);
                controller.close_with_epitaph(zx::Status::INVALID_ARGS).ok();
                return;
            };

            let Ok(token) = component_instance.duplicate_handle(zx::Rights::SAME_RIGHTS) else {
                error!("Failed to duplicate component instance for driver '{}'", url);
                controller.close_with_epitaph(zx::Status::INTERNAL).ok();
                return;
            };

            match self.introspector.get_moniker(token).await {
                Ok(Ok(moniker)) => {
                    // Moniker string is like "realm/child". We need just "child" (collection/name)?
                    // Actually Introspector returns relative moniker.
                    // C++ code does: if (split_point <= 0) error; moniker = moniker.substr(split_point + 1);
                    // Assuming moniker is "collection:name" or "name"?
                    // Wait, C++ says `moniker.find(':')`. That implies "collection:name".
                    // Let's verify what `GetMoniker` returns.
                    // The C++ code strips the collection name.
                    if let Some(pos) = moniker.find(':') {
                        moniker[pos + 1..].to_string()
                    } else {
                        error!(
                            "Moniker '{}' for driver '{}' does not contain collection",
                            moniker, url
                        );
                        controller.close_with_epitaph(zx::Status::INVALID_ARGS).ok();
                        return;
                    }
                }
                Ok(Err(e)) => {
                    error!("Failed to get moniker for driver '{}': {:?}", url, e);
                    controller.close_with_epitaph(zx::Status::INTERNAL).ok();
                    return;
                }
                Err(e) => {
                    error!("Failed to call GetMoniker for driver '{}': {}", url, e);
                    controller.close_with_epitaph(zx::Status::INTERNAL).ok();
                    return;
                }
            }
        };

        if self.pending_starts.borrow().get(&moniker).is_none() {
            error!("Failed to start driver '{}', unknown request for driver {}", url, moniker);
            controller.close_with_epitaph(zx::Status::UNAVAILABLE).ok();
            return;
        }

        self.complete_request(&moniker, Ok(StartedComponent { info: start_info, controller }));
    }
}
