// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::realm::{Durability, get_all_instances, get_resolved_declaration};
use anyhow::{Result, bail};
use cm_rust::offer::{OfferDeclCommon, OfferTarget};
use flex_fuchsia_sys2 as fsys;
use moniker::Moniker;
use prettytable::format::consts::FORMAT_CLEAN;
use prettytable::{Table, row};

struct Collection {
    name: String,
    moniker: Moniker,
    durability: Durability,
    environment: Option<String>,
    offered_capabilities: Vec<String>,
}

pub async fn collection_list_cmd<W: std::io::Write>(
    realm_query: fsys::RealmQueryProxy,
    mut writer: W,
) -> Result<()> {
    let collections = get_all_collections(&realm_query).await?;

    let table = create_table(collections);
    table.print(&mut writer)?;

    Ok(())
}

pub async fn collection_show_cmd<W: std::io::Write>(
    query: String,
    realm_query: fsys::RealmQueryProxy,
    mut writer: W,
) -> Result<()> {
    let collections = get_all_collections(&realm_query).await?;

    let filtered_collections: Vec<Collection> =
        collections.into_iter().filter(|c| c.name.contains(&query)).collect();

    if filtered_collections.is_empty() {
        bail!("No collections found for query \"{}\"", query);
    }

    for collection in filtered_collections {
        let table = create_verbose_table(&collection);
        table.print(&mut writer)?;
        writeln!(writer, "")?;
    }

    Ok(())
}

async fn get_all_collections(realm_query: &fsys::RealmQueryProxy) -> Result<Vec<Collection>> {
    let instances = get_all_instances(realm_query).await?;
    let mut collections = vec![];

    for instance in instances {
        if instance.resolved_info.is_some() {
            let mut instance_collections =
                get_all_collections_of_instance(&instance.moniker, realm_query).await?;
            collections.append(&mut instance_collections);
        }
    }

    Ok(collections)
}

async fn get_all_collections_of_instance(
    moniker: &Moniker,
    realm_query: &fsys::RealmQueryProxy,
) -> Result<Vec<Collection>> {
    let manifest = get_resolved_declaration(moniker, realm_query).await?;
    let mut collections = vec![];

    for collection in manifest.collections {
        let mut offered_capabilities = vec![];

        for offer in &manifest.offers {
            match offer.target() {
                OfferTarget::Collection(name) => {
                    if name == &collection.name {
                        offered_capabilities.push(offer.target_name().to_string());
                    }
                }
                _ => {}
            }
        }

        collections.push(Collection {
            name: collection.name.to_string(),
            moniker: moniker.clone(),
            durability: collection.durability.into(),
            environment: collection.environment.map(|e| e.to_string()),
            offered_capabilities,
        });
    }

    Ok(collections)
}

fn create_table(collections: Vec<Collection>) -> Table {
    let mut table = Table::new();
    table.set_format(*FORMAT_CLEAN);
    table.set_titles(row!("Moniker", "Name", "Durability", "Environment"));

    for collection in collections {
        let environment = collection.environment.unwrap_or_else(|| "N/A".to_string());
        table.add_row(row!(
            collection.moniker.to_string(),
            collection.name,
            collection.durability.to_string(),
            environment
        ));
    }

    table
}

fn create_verbose_table(collection: &Collection) -> Table {
    let mut table = Table::new();
    table.set_format(*FORMAT_CLEAN);
    table.add_row(row!(r->"Moniker:", collection.moniker.to_string()));
    table.add_row(row!(r->"Collection Name:", collection.name));
    table.add_row(row!(r->"Durability:", collection.durability.to_string()));

    let environment = collection.environment.clone().unwrap_or_else(|| "N/A".to_string());
    table.add_row(row!(r->"Environment:", environment));

    let offered_capabilities = collection.offered_capabilities.join("\n");
    table.add_row(row!(r->"Offered Capabilities:", offered_capabilities));

    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use fidl_fuchsia_component_decl as fdecl;
    use std::collections::HashMap;

    fn create_query() -> fsys::RealmQueryProxy {
        // Serve RealmQuery for CML components.
        let query = serve_realm_query(
            vec![fsys::Instance {
                moniker: Some("./my_foo".to_string()),
                url: Some("fuchsia-pkg://fuchsia.com/foo#meta/foo.cm".to_string()),
                instance_id: Some("1234567890".to_string()),
                resolved_info: Some(fsys::ResolvedInfo {
                    resolved_url: Some("fuchsia-pkg://fuchsia.com/foo#meta/foo.cm".to_string()),
                    execution_info: None,
                    ..Default::default()
                }),
                ..Default::default()
            }],
            HashMap::from([(
                "./my_foo".to_string(),
                fdecl::Component {
                    offers: Some(vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                        source_name: Some("fuchsia.foo.bar".to_string()),
                        target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll1".to_string(),
                        })),
                        target_name: Some("fuchsia.foo.bar".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    })]),
                    collections: Some(vec![fdecl::Collection {
                        name: Some("coll1".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    }]),
                    ..Default::default()
                },
            )]),
            HashMap::from([]),
            HashMap::from([]),
        );
        query
    }

    #[fuchsia::test]
    async fn get_all_collections_test() {
        let query = create_query();

        let mut collections = get_all_collections(&query).await.unwrap();

        assert_eq!(collections.len(), 1);

        let collection = collections.remove(0);

        assert_eq!(collection.name, "coll1");
        assert_eq!(collection.moniker, Moniker::parse_str("/my_foo").unwrap());
        assert_eq!(collection.durability, Durability::Transient);
        assert!(collection.environment.is_none());
        assert_eq!(collection.offered_capabilities, vec!["fuchsia.foo.bar"]);
    }
}
