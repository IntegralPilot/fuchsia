// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_rust::offer::OfferDeclCommon;
use cm_rust::{ExposeDeclCommon, NativeIntoFidl};
use flex_client::ProxyHasDomain;
use flex_client::fidl::{ClientEnd, DiscoverableProtocolMarker};
use flex_fuchsia_component as fcomponent;
use flex_fuchsia_component_decl::Offer;
use flex_fuchsia_io as fio;
use flex_fuchsia_sys2 as fsys;
use moniker::Moniker;
use thiserror::Error;
const CAPABILITY_REQUESTED_EVENT: &str = "capability_requested";

#[derive(Debug, Error)]
pub enum RealmValidationError {
    #[error("Realm should expose {}", fcomponent::RealmMarker::PROTOCOL_NAME)]
    RealmProtocol,

    #[error(
        "Realm should offer {} event stream to the test collection",
        CAPABILITY_REQUESTED_EVENT
    )]
    CapabilityRequested,

    #[error("The realm does not contain '{0}' named collection")]
    TestCollectionNotFound(String),
}

#[derive(Debug, Error)]
pub enum RealmError {
    #[error(transparent)]
    Fidl(#[from] fidl::Error),

    #[error(transparent)]
    Validation(#[from] RealmValidationError),

    #[error("Invalid realm, it should contain test collection: /realm/collection")]
    InvalidRealmStr,

    #[error("cannot resolve provided realm: {0:?}")]
    InstanceNotResolved(component_debug::lifecycle::ResolveError),

    #[error(transparent)]
    BadMoniker(#[from] moniker::MonikerError),

    #[error("Cannot connect to exposed directory: {0:?}")]
    ConnectExposedDir(fsys::OpenError),

    #[error(transparent)]
    GetManifest(#[from] component_debug::realm::GetDeclarationError),
}

pub struct Realm {
    exposed_dir: fio::DirectoryProxy,
    offers: Vec<flex_fuchsia_component_decl::Offer>,
    realm_str: String,
    test_collection: String,
}

impl std::fmt::Debug for Realm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("realm for moniker {}:{}", self.realm_str, self.test_collection))
    }
}

impl PartialEq for Realm {
    fn eq(&self, other: &Self) -> bool {
        self.realm_str == other.realm_str && self.test_collection == other.test_collection
    }
}

impl Realm {
    pub fn get_realm_client(&self) -> Result<ClientEnd<fcomponent::RealmMarker>, fidl::Error> {
        let (realm_client, server_end) =
            self.exposed_dir.domain().create_endpoints::<fcomponent::RealmMarker>();
        self.exposed_dir.open(
            fcomponent::RealmMarker::PROTOCOL_NAME,
            fio::Flags::PROTOCOL_SERVICE,
            &Default::default(),
            server_end.into_channel(),
        )?;
        Ok(realm_client)
    }

    pub fn offers(&self) -> Vec<flex_fuchsia_component_decl::Offer> {
        self.offers.clone()
    }

    pub fn collection<'a>(&'a self) -> &'a str {
        self.test_collection.as_str()
    }
}

fn validate_and_get_offers(
    manifest: cm_rust::ComponentDecl,
    test_collection: &str,
) -> Result<Vec<Offer>, RealmValidationError> {
    let collection_found = manifest.collections.iter().any(|c| c.name == test_collection);
    if !collection_found {
        return Err(RealmValidationError::TestCollectionNotFound(test_collection.to_string()));
    }

    let exposes_realm_protocol =
        manifest.exposes.iter().any(|e| *e.target_name() == fcomponent::RealmMarker::PROTOCOL_NAME);
    if !exposes_realm_protocol {
        return Err(RealmValidationError::RealmProtocol);
    }

    let mut capability_requested = false;
    let mut offers = vec![];
    for offer in manifest.offers {
        if let cm_rust::offer::OfferTarget::Collection(collection) = &offer.target() {
            if collection.as_str() != test_collection {
                continue;
            }

            if let cm_rust::offer::OfferDecl::EventStream(decl) = &offer {
                let cm_rust::offer::OfferEventStreamDecl { target_name, source, scope, .. } =
                    &**decl;

                if *target_name == CAPABILITY_REQUESTED_EVENT
                    && source == &cm_rust::offer::OfferSource::Parent
                    && scope
                        .as_ref()
                        .map(|s| {
                            s.iter().any(|s| match s {
                                cm_rust::EventScope::Collection(s) => s.as_str() == test_collection,
                                _ => false,
                            })
                        })
                        .unwrap_or(false)
                {
                    capability_requested =
                        capability_requested || *target_name == CAPABILITY_REQUESTED_EVENT;
                }
            }
            offers.push(offer.native_into_fidl());
        }
    }
    if !capability_requested {
        return Err(RealmValidationError::CapabilityRequested);
    }
    Ok(offers)
}

pub async fn parse_provided_realm(
    lifecycle_controller: &fsys::LifecycleControllerProxy,
    realm_query: &fsys::RealmQueryProxy,
    realm_str: &str,
) -> Result<Realm, RealmError> {
    let (mut moniker, mut test_collection) = match realm_str.rsplit_once('/') {
        Some(s) => s,
        None => {
            return Err(RealmError::InvalidRealmStr);
        }
    };
    // Support old way of parsing realm.
    if test_collection.contains(":") {
        (moniker, test_collection) = match realm_str.rsplit_once(':') {
            Some(s @ (moniker_head, collection_name)) => {
                eprintln!(
                    "You are using old realm format. Please switch to standard realm moniker format: '{}/{}'",
                    moniker_head, collection_name
                );
                s
            }
            None => {
                return Err(RealmError::InvalidRealmStr);
            }
        };
    }
    if moniker == "" {
        return Err(RealmError::InvalidRealmStr);
    }
    let moniker = Moniker::try_from(moniker)?;

    component_debug::lifecycle::resolve_instance(&lifecycle_controller, &moniker)
        .await
        .map_err(RealmError::InstanceNotResolved)?;

    let manifest = component_debug::realm::get_resolved_declaration(&moniker, &realm_query).await?;

    let offers = validate_and_get_offers(manifest, test_collection)?;

    let (exposed_dir, server_end) = realm_query.domain().create_proxy();
    realm_query
        .open_directory(moniker.as_ref(), fsys::OpenDirType::ExposedDir, server_end)
        .await?
        .map_err(RealmError::ConnectExposedDir)?;

    Ok(Realm {
        exposed_dir,
        offers,
        realm_str: realm_str.to_string(),
        test_collection: test_collection.to_owned(),
    })
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;
    use cm_rust::FidlIntoNative;

    #[fuchsia::test]
    async fn valid_realm() {
        let lifecycle_controller =
            fuchsia_component::client::connect_to_protocol::<fsys::LifecycleControllerMarker>()
                .unwrap();
        let realm_query =
            fuchsia_component::client::connect_to_protocol::<fsys::RealmQueryMarker>().unwrap();
        let realm =
            parse_provided_realm(&lifecycle_controller, &realm_query, "/test_realm/echo_test_coll")
                .await
                .unwrap();

        assert_eq!(realm.test_collection, "echo_test_coll");
        assert_eq!(realm.realm_str, "/test_realm/echo_test_coll");

        let offers: Vec<cm_rust::OfferDecl> =
            realm.offers.into_iter().map(|o| o.fidl_into_native()).collect::<Vec<_>>();
        assert_eq!(offers.len(), 3, "{:?}", offers);
        offers.iter().for_each(|o| {
            assert_eq!(
                o.target(),
                &cm_rust::offer::OfferTarget::Collection("echo_test_coll".parse().unwrap())
            )
        });
        assert!(offers.iter().any(|o| *o.target_name() == CAPABILITY_REQUESTED_EVENT));
        assert!(offers.iter().any(|o| *o.target_name() == "fidl.examples.routing.echo.Echo"));

        let realm = parse_provided_realm(
            &lifecycle_controller,
            &realm_query,
            "/test_realm/hermetic_test_coll",
        )
        .await
        .unwrap();

        assert_eq!(realm.test_collection, "hermetic_test_coll");
        assert_eq!(realm.realm_str, "/test_realm/hermetic_test_coll");

        let offers = realm.offers.into_iter().map(|o| o.fidl_into_native()).collect::<Vec<_>>();
        assert_eq!(offers.len(), 2, "{:?}", offers);
        offers.iter().for_each(|o: &cm_rust::OfferDecl| {
            assert_eq!(
                o.target(),
                &cm_rust::offer::OfferTarget::Collection("hermetic_test_coll".parse().unwrap())
            )
        });
        assert!(offers.iter().any(|o| *o.target_name() == CAPABILITY_REQUESTED_EVENT));
    }

    #[fuchsia::test]
    async fn invalid_realm() {
        let lifecycle_controller =
            fuchsia_component::client::connect_to_protocol::<fsys::LifecycleControllerMarker>()
                .unwrap();
        let realm_query =
            fuchsia_component::client::connect_to_protocol::<fsys::RealmQueryMarker>().unwrap();
        assert_matches!(
            parse_provided_realm(
                &lifecycle_controller,
                &realm_query,
                "/nonexistent_realm/test_coll"
            )
            .await,
            Err(RealmError::InstanceNotResolved(_))
        );

        assert_matches!(
            parse_provided_realm(&lifecycle_controller, &realm_query, "/test_realm").await,
            Err(RealmError::InvalidRealmStr)
        );

        assert_matches!(
            parse_provided_realm(&lifecycle_controller, &realm_query, "/test_realm/invalid_col")
                .await,
            Err(RealmError::Validation(RealmValidationError::TestCollectionNotFound(_)))
        );

        assert_matches!(
            parse_provided_realm(
                &lifecycle_controller,
                &realm_query,
                "/test_realm/no_capability_requested_event"
            )
            .await,
            Err(RealmError::Validation(RealmValidationError::CapabilityRequested))
        );

        assert_matches!(
            parse_provided_realm(
                &lifecycle_controller,
                &realm_query,
                "/no_realm_protocol_realm/hermetic_test_coll"
            )
            .await,
            Err(RealmError::Validation(RealmValidationError::RealmProtocol))
        );
    }
}
