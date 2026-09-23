// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::commands::types::DiagnosticsProvider;
use crate::types::Error;
use anyhow::anyhow;
use cm_rust::{ExposeDeclCommon, ExposeSource, SourceName};

#[cfg(not(feature = "fdomain"))]
use component_debug;
#[cfg(feature = "fdomain")]
use component_debug_fdomain as component_debug;

use component_debug::dirs::*;
use component_debug::realm::*;
use flex_client::ProxyHasDomain;
use flex_client::fidl::DiscoverableProtocolMarker;
use flex_fuchsia_diagnostics::{All, ArchiveAccessorMarker, Selector, TreeNames};

#[cfg(not(feature = "fdomain"))]
use fuchsia_fs;
#[cfg(feature = "fdomain")]
use fuchsia_fs_fdomain as fuchsia_fs;

use flex_fuchsia_io as fio;
use flex_fuchsia_sys2 as fsys2;
use fuchsia_fs::directory;
use moniker::Moniker;

const ACCESSORS_DICTIONARY: &str = "diagnostics-accessors";

/// Attempt to connect to the `fuchsia.diagnostics.*ArchiveAccessor` with the selector
/// specified.
pub async fn connect_accessor<P: DiscoverableProtocolMarker>(
    moniker: &Moniker,
    accessor_name: &str,
    proxy: &fsys2::RealmQueryProxy,
) -> Result<P::Proxy, Error> {
    let proxy = connect_to_instance_protocol_at_path::<P>(
        moniker,
        OpenDirType::Exposed,
        &format!("{ACCESSORS_DICTIONARY}/{accessor_name}"),
        proxy,
    )
    .await
    .map_err(|e| Error::ConnectToProtocol(accessor_name.to_string(), anyhow!("{:?}", e)))?;
    Ok(proxy)
}

async fn list_accessors(
    realm: &fsys2::RealmQueryProxy,
    moniker: &Moniker,
) -> Option<Vec<fuchsia_fs::directory::DirEntry>> {
    let Ok(exposed_dir) = open_instance_directory(moniker, OpenDirType::Exposed, realm).await
    else {
        return None;
    };

    let (dir_proxy, server_end) = realm.domain().create_proxy::<fio::DirectoryMarker>();
    exposed_dir
        .open(
            ACCESSORS_DICTIONARY,
            fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE,
            &Default::default(),
            server_end.into_channel(),
        )
        .expect("Open call failed!");

    directory::readdir(&dir_proxy).await.ok()
}

/// Searches for a diagnostics `ArchiveAccessor` across all components in the
/// topology.
///
/// Unlike [`fuzzy_search`], which queries component instances by their moniker,
/// URL, or ID, this function specifically inspects capabilities exposed under
/// the `diagnostics-accessors` dictionary starting with
/// `fuchsia.diagnostics.ArchiveAccessor`.
///
/// Query format:
/// - `<moniker_query>:<protocol_query>`: Fuzzy matches on both the component
///   moniker and the accessor protocol name.
/// - `<moniker_query>`: Fuzzy matches on the moniker and matches any accessor
///   protocol exposed by that component.
///
/// Disambiguation:
/// - Exact moniker matches take precedence over partial substring moniker
///   matches.
/// - Exact protocol matches take precedence over partial substring protocol
///   matches.
/// - If resolution cannot be narrowed to a single accessor, returns
///   [`Error::FuzzyMatchTooManyMatches`].
pub async fn fuzzy_search_accessors(
    query: &str,
    realm_query: &fsys2::RealmQueryProxy,
) -> Result<(Moniker, String), Error> {
    let (moniker_query, protocol_query) = query.rsplit_once(':').unwrap_or((query, ""));
    let mut matching_monikers_and_protocols: Vec<_> = get_accessor_selectors(realm_query)
        .await?
        .iter()
        .filter_map(|i| i.rsplit_once(":"))
        .filter(|(m, p)| m.contains(moniker_query) && p.contains(protocol_query))
        .filter_map(|(m, p)| Moniker::parse_str(m).ok().map(|moniker| (moniker, p.to_string())))
        .collect();
    if matching_monikers_and_protocols.is_empty() {
        return Err(Error::SearchParameterNotFound(query.to_string()));
    } else if matching_monikers_and_protocols.len() > 1 {
        let mut moniker_exact_match: Vec<_> = matching_monikers_and_protocols
            .iter()
            .filter(|(m, _)| m.to_string() == moniker_query)
            .cloned()
            .collect();
        // Return early if the moniker uniquely matches exactly one component.
        if moniker_exact_match.len() == 1 {
            return Ok(moniker_exact_match.pop().unwrap());
        } else if moniker_exact_match.len() > 1 {
            // If multiple accessors match under the exact moniker, disambiguate by exact protocol.
            moniker_exact_match.retain(|(_, p)| p == protocol_query);
            if moniker_exact_match.len() == 1 {
                return Ok(moniker_exact_match.pop().unwrap());
            }
        }

        // If the moniker was a fuzzy query, attempt to disambiguate by exact protocol across
        // components.
        let mut protocol_exact_match: Vec<_> = matching_monikers_and_protocols
            .iter()
            .filter(|(_, p)| p == protocol_query)
            .cloned()
            .collect();
        if protocol_exact_match.len() == 1 {
            return Ok(protocol_exact_match.pop().unwrap());
        }

        return Err(Error::FuzzyMatchTooManyMatches(
            matching_monikers_and_protocols.into_iter().map(|(m, p)| format!("{m}:{p}")).collect(),
        ));
    }

    Ok(matching_monikers_and_protocols.pop().unwrap())
}

/// Searches for a single component instance in the topology matching `query`.
///
/// Unlike [`fuzzy_search_accessors`], which inspects exposed diagnostics
/// accessor protocols, this function matches `query` against component
/// identifiers (moniker, component URL, and instance ID) across all instances
/// in the realm without inspecting exposed capabilities or protocols.
///
/// If multiple instances match, returns [`Error::FuzzyMatchTooManyMatches`].
async fn fuzzy_search(
    query: &str,
    realm_query: &fsys2::RealmQueryProxy,
) -> Result<Instance, Error> {
    let mut instances = component_debug::query::get_instances_from_query(query, realm_query)
        .await
        .map_err(Error::FuzzyMatchRealmQuery)?;
    if instances.is_empty() {
        return Err(Error::SearchParameterNotFound(query.to_string()));
    } else if instances.len() > 1 {
        return Err(Error::FuzzyMatchTooManyMatches(
            instances.into_iter().map(|i| i.moniker.to_string()).collect(),
        ));
    }

    Ok(instances.pop().unwrap())
}

pub async fn process_fuzzy_inputs<P: DiagnosticsProvider>(
    queries: impl IntoIterator<Item = String>,
    provider: &P,
) -> Result<Vec<Selector>, Error> {
    let mut queries = queries.into_iter().peekable();
    if queries.peek().is_none() {
        return Ok(vec![]);
    }

    let realm_query = provider.realm_query();
    let mut results = vec![];
    for value in queries {
        match fuzzy_search(&value, realm_query).await {
            // try again in case this is a fully escaped moniker or selector
            Err(Error::SearchParameterNotFound(_)) => {
                // In case they included a tree-selector segment, attempt to parse but don't bail
                // on failure
                if let Ok(selector) = selectors::parse_verbose(&value) {
                    results.push(selector);
                } else {
                    // Note the lack of `sanitize_moniker_for_selectors`. `value` is assumed to
                    // either
                    //   A) Be a component that isn't running; therefore the selector being
                    //      right or wrong is irrelevant
                    //   B) Already be sanitized by the caller
                    let selector_string = format!("{value}:root");
                    results.push(
                        selectors::parse_verbose(&selector_string)
                            .map_err(|e| Error::ParseSelector(selector_string, e.into()))?,
                    )
                }
            }
            Err(e) => return Err(e),
            Ok(instance) => {
                let selector_string = format!(
                    "{}:root",
                    selectors::sanitize_moniker_for_selectors(instance.moniker.as_ref()),
                );
                results.push(
                    selectors::parse_verbose(&selector_string)
                        .map_err(|e| Error::ParseSelector(selector_string, e.into()))?,
                )
            }
        }
    }

    Ok(results)
}

/// Returns the selectors for a component whose url, manifest, or moniker contains the
/// `component` string.
pub async fn process_component_query_with_partial_selectors<P: DiagnosticsProvider>(
    component: &str,
    tree_selectors: impl Iterator<Item = String>,
    provider: &P,
) -> Result<Vec<Selector>, Error> {
    let mut tree_selectors = tree_selectors.into_iter().peekable();
    let realm_query = provider.realm_query();
    let instance = fuzzy_search(component, realm_query).await?;

    let mut results = vec![];
    if tree_selectors.peek().is_none() {
        let selector_string = format!(
            "{}:root",
            selectors::sanitize_moniker_for_selectors(instance.moniker.as_ref())
        );
        results
            .push(selectors::parse_verbose(&selector_string).map_err(Error::PartialSelectorHint)?);
    } else {
        for s in tree_selectors {
            let selector_string = format!(
                "{}:{}",
                selectors::sanitize_moniker_for_selectors(instance.moniker.as_ref()),
                s
            );
            results.push(
                selectors::parse_verbose(&selector_string).map_err(Error::PartialSelectorHint)?,
            )
        }
    }

    Ok(results)
}

fn add_tree_name(selector: &mut Selector, tree_name: String) -> Result<(), Error> {
    match selector.tree_names {
        None => selector.tree_names = Some(TreeNames::Some(vec![tree_name])),
        Some(ref mut names) => match names {
            TreeNames::Some(names) => {
                if !names.iter().any(|n| n == &tree_name) {
                    names.push(tree_name)
                }
            }
            TreeNames::All(_) => {}
            TreeNames::__SourceBreaking { unknown_ordinal } => {
                let unknown_ordinal = *unknown_ordinal;
                return Err(Error::InvalidSelector(format!(
                    "selector had invalid TreeNames variant {unknown_ordinal}: {selector:?}",
                )));
            }
        },
    }
    Ok(())
}

/// Expand selectors with a tree name. If a tree name is given, the selectors will be guaranteed to
/// include the tree name given unless they already have a tree name set. If no tree name is given
/// and the selectors carry no tree name, then they'll be updated to target all tree names
/// associated with the component.
pub fn ensure_tree_field_is_set(
    selectors: &mut Vec<Selector>,
    tree_name: Option<String>,
) -> Result<(), Error> {
    if selectors.is_empty() {
        let Some(tree_name) = tree_name else {
            return Ok(());
        };

        // Safety: "**:*" is a valid selector
        let mut selector = selectors::parse_verbose("**:*").unwrap();
        selector.tree_names = Some(TreeNames::Some(vec![tree_name]));
        selectors.push(selector);
        return Ok(());
    }

    for selector in selectors.iter_mut() {
        if let Some(tree_name) = &tree_name {
            add_tree_name(selector, tree_name.clone())?;
        } else if selector.tree_names.is_none() {
            selector.tree_names = Some(TreeNames::All(All {}))
        }
    }

    Ok(())
}

/// Get all the exposed `ArchiveAccessor` from any child component which
/// directly exposes them or places them in its outgoing directory.
pub async fn get_accessor_selectors(
    realm_query: &fsys2::RealmQueryProxy,
) -> Result<Vec<String>, Error> {
    let mut result = vec![];
    let instances = get_all_instances(realm_query).await?;
    for instance in instances {
        match get_resolved_declaration(&instance.moniker, realm_query).await {
            Err(GetDeclarationError::InstanceNotFound(_))
            | Err(GetDeclarationError::InstanceNotResolved(_)) => continue,
            Err(err) => return Err(err.into()),
            Ok(decl) => {
                for capability in decl.capabilities {
                    let capability_name = capability.name().to_string();
                    if capability_name != ACCESSORS_DICTIONARY {
                        continue;
                    }
                    if !decl.exposes.iter().any(|expose| {
                        expose.source_name() == capability.name()
                            && *expose.source() == ExposeSource::Self_
                    }) {
                        continue;
                    }

                    let Some(entries) = list_accessors(realm_query, &instance.moniker).await else {
                        continue;
                    };

                    for entry in entries {
                        let directory::DirEntry { name, kind: fio::DirentType::Service } = entry
                        else {
                            continue;
                        };
                        // This skips .host accessors intentionally.
                        if !name.starts_with(ArchiveAccessorMarker::PROTOCOL_NAME) {
                            continue;
                        }
                        result.push(format!("{}:{name}", instance.moniker));
                    }
                }
            }
        }
    }
    result.sort();
    Ok(result)
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;
    use iquery_test_support::{MockRealmQuery, MockRealmQueryBuilder};
    use selectors::parse_verbose;
    use std::rc::Rc;

    #[fuchsia::test]
    async fn test_get_accessors() {
        let fake_realm_query = Rc::new(MockRealmQuery::default());
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        let res = get_accessor_selectors(&realm_query).await;

        assert_matches!(res, Ok(_));

        assert_eq!(
            res.unwrap(),
            vec![
                String::from("example/component:fuchsia.diagnostics.ArchiveAccessor"),
                String::from("foo/bar/thing:instance:fuchsia.diagnostics.ArchiveAccessor.feedback"),
                String::from("foo/component:fuchsia.diagnostics.ArchiveAccessor.feedback"),
            ]
        );
    }

    #[fuchsia::test]
    fn test_ensure_tree_field_is_set() {
        let name = Some("abc".to_string());
        let expected = vec![
            parse_verbose("core/one:[name=abc]root").unwrap(),
            parse_verbose("core/one:[name=xyz, name=abc]root").unwrap(),
        ];

        let mut actual = vec![
            parse_verbose("core/one:root").unwrap(),
            parse_verbose("core/one:[name=xyz]root").unwrap(),
        ];
        ensure_tree_field_is_set(&mut actual, name.clone()).unwrap();
        assert_eq!(actual, expected);
    }

    #[fuchsia::test]
    fn test_ensure_tree_field_is_set_noop_when_tree_names_set() {
        let expected = vec![
            parse_verbose("core/one:[...]root").unwrap(),
            parse_verbose("core/one:[name=xyz]root").unwrap(),
        ];
        let mut actual = vec![
            parse_verbose("core/one:root").unwrap(),
            parse_verbose("core/one:[name=xyz]root").unwrap(),
        ];
        ensure_tree_field_is_set(&mut actual, None).unwrap();
        assert_eq!(actual, expected);
    }

    #[fuchsia::test]
    fn test_ensure_tree_field_is_set_noop_on_empty_vec_no_name() {
        let mut actual = vec![];
        ensure_tree_field_is_set(&mut actual, None).unwrap();
        assert_eq!(actual, vec![]);
    }

    #[fuchsia::test]
    fn test_ensure_tree_field_is_set_all_components_when_empty_and_name() {
        let expected = vec![parse_verbose("**:[name=abc]*").unwrap()];
        let mut actual = vec![];
        let name = Some("abc".to_string());
        ensure_tree_field_is_set(&mut actual, name).unwrap();
        assert_eq!(actual, expected);
    }

    struct FakeProvider {
        realm_query: fsys2::RealmQueryProxy,
    }

    impl FakeProvider {
        async fn new(monikers: &'static [&'static str]) -> Self {
            let mut builder = MockRealmQueryBuilder::default();
            for name in monikers {
                builder = builder.when(name).moniker(name).add();
            }
            let realm_query_proxy = Rc::new(builder.build()).get_proxy().await;
            Self { realm_query: realm_query_proxy }
        }
    }

    impl DiagnosticsProvider for FakeProvider {
        async fn snapshot(
            &self,
            _: Option<&str>,
            _: impl IntoIterator<Item = Selector>,
        ) -> Result<Vec<diagnostics_data::Data<diagnostics_data::Inspect>>, Error> {
            unreachable!("unimplemented");
        }

        async fn get_accessor_paths(&self) -> Result<Vec<String>, Error> {
            unreachable!("unimplemented");
        }

        fn realm_query(&self) -> &fsys2::RealmQueryProxy {
            &self.realm_query
        }
    }

    #[fuchsia::test]
    async fn test_process_fuzzy_inputs_success() {
        let actual = process_fuzzy_inputs(
            ["moniker1".to_string()],
            &FakeProvider::new(&["core/moniker1", "core/moniker2"]).await,
        )
        .await
        .unwrap();

        let expected = vec![parse_verbose("core/moniker1:root").unwrap()];

        assert_eq!(actual, expected);

        let actual = process_fuzzy_inputs(
            ["moniker1:collection".to_string()],
            &FakeProvider::new(&["core/moniker1:collection", "core/moniker1", "core/moniker2"])
                .await,
        )
        .await
        .unwrap();

        let expected = vec![parse_verbose(r"core/moniker1\:collection:root").unwrap()];

        assert_eq!(actual, expected);

        let actual = process_fuzzy_inputs(
            [r"core/moniker1\:collection".to_string()],
            &FakeProvider::new(&["core/moniker1:collection"]).await,
        )
        .await
        .unwrap();

        let expected = vec![parse_verbose(r"core/moniker1\:collection:root").unwrap()];

        assert_eq!(actual, expected);

        let actual = process_fuzzy_inputs(
            ["core/moniker1:root:prop".to_string()],
            &FakeProvider::new(&["core/moniker1:collection", "core/moniker1"]).await,
        )
        .await
        .unwrap();

        let expected = vec![parse_verbose(r"core/moniker1:root:prop").unwrap()];

        assert_eq!(actual, expected);

        let actual = process_fuzzy_inputs(
            ["core/moniker1".to_string(), "core/moniker2".to_string()],
            &FakeProvider::new(&["core/moniker1", "core/moniker2"]).await,
        )
        .await
        .unwrap();

        let expected = vec![
            parse_verbose(r"core/moniker1:root").unwrap(),
            parse_verbose(r"core/moniker2:root").unwrap(),
        ];

        assert_eq!(actual, expected);

        let actual = process_fuzzy_inputs(
            ["moniker1".to_string(), "moniker2".to_string()],
            &FakeProvider::new(&["core/moniker1"]).await,
        )
        .await
        .unwrap();

        let expected = vec![
            parse_verbose(r"core/moniker1:root").unwrap(),
            // fallback is to assume that moniker2 is a valid moniker
            parse_verbose("moniker2:root").unwrap(),
        ];

        assert_eq!(actual, expected);

        let actual = process_fuzzy_inputs(
            ["core/moniker1:root:prop".to_string(), "core/moniker2".to_string()],
            &FakeProvider::new(&["core/moniker1", "core/moniker2"]).await,
        )
        .await
        .unwrap();

        let expected = vec![
            parse_verbose(r"core/moniker1:root:prop").unwrap(),
            parse_verbose(r"core/moniker2:root").unwrap(),
        ];

        assert_eq!(actual, expected);
    }

    #[fuchsia::test]
    async fn test_process_fuzzy_inputs_failures() {
        let actual =
            process_fuzzy_inputs(["moniker ".to_string()], &FakeProvider::new(&["moniker"]).await)
                .await;

        assert_matches!(actual, Err(Error::ParseSelector(_, _)));

        let actual = process_fuzzy_inputs(
            ["moniker".to_string()],
            &FakeProvider::new(&["core/moniker1", "core/moniker2"]).await,
        )
        .await;

        assert_matches!(actual, Err(Error::FuzzyMatchTooManyMatches(_)));
    }

    #[fuchsia::test]
    async fn test_fuzzy_component_search() {
        let actual = process_component_query_with_partial_selectors(
            "moniker1",
            [].into_iter(),
            &FakeProvider::new(&["core/moniker1", "core/moniker2"]).await,
        )
        .await
        .unwrap();

        let expected = vec![parse_verbose(r"core/moniker1:root").unwrap()];

        assert_eq!(actual, expected);

        let actual = process_component_query_with_partial_selectors(
            "moniker1",
            ["root/foo:bar".to_string()].into_iter(),
            &FakeProvider::new(&["core/moniker1", "core/moniker2"]).await,
        )
        .await
        .unwrap();

        let expected = vec![parse_verbose(r"core/moniker1:root/foo:bar").unwrap()];

        assert_eq!(actual, expected);

        let actual = process_component_query_with_partial_selectors(
            "moniker1",
            ["root/foo:bar".to_string()].into_iter(),
            &FakeProvider::new(&["core/moniker2", "core/moniker3"]).await,
        )
        .await;

        assert_matches!(actual, Err(Error::SearchParameterNotFound(_)));
    }

    #[fuchsia::test]
    async fn test_fuzzy_search_accessors_exact_match() {
        let fake_realm_query = Rc::new(MockRealmQuery::default());
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        let (moniker, protocol) = fuzzy_search_accessors(
            "example/component:fuchsia.diagnostics.ArchiveAccessor",
            &realm_query,
        )
        .await
        .unwrap();
        assert_eq!(moniker, Moniker::parse_str("example/component").unwrap());
        assert_eq!(protocol, "fuchsia.diagnostics.ArchiveAccessor");
    }

    #[fuchsia::test]
    async fn test_fuzzy_search_accessors_fuzzy_match_moniker_and_protocol() {
        let fake_realm_query = Rc::new(MockRealmQuery::default());
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        let (moniker, protocol) =
            fuzzy_search_accessors("example:ArchiveAccessor", &realm_query).await.unwrap();
        assert_eq!(moniker, Moniker::parse_str("example/component").unwrap());
        assert_eq!(protocol, "fuchsia.diagnostics.ArchiveAccessor");
    }

    #[fuchsia::test]
    async fn test_fuzzy_search_accessors_moniker_with_colon() {
        let fake_realm_query = Rc::new(MockRealmQuery::default());
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        let (moniker, protocol) =
            fuzzy_search_accessors("thing:instance:feedback", &realm_query).await.unwrap();
        assert_eq!(moniker, Moniker::parse_str("foo/bar/thing:instance").unwrap());
        assert_eq!(protocol, "fuchsia.diagnostics.ArchiveAccessor.feedback");
    }

    #[fuchsia::test]
    async fn test_fuzzy_search_accessors_disambiguation_by_exact_moniker() {
        let fake_realm_query = Rc::new(
            MockRealmQueryBuilder::new()
                .when("example/component")
                .moniker("./example/component")
                .accessors(&["fuchsia.diagnostics.ArchiveAccessor"])
                .add()
                .when("example/component/subcomponent")
                .moniker("./example/component/subcomponent")
                .accessors(&["fuchsia.diagnostics.ArchiveAccessor"])
                .add()
                .build(),
        );
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        let (moniker, protocol) = fuzzy_search_accessors(
            "example/component:fuchsia.diagnostics.ArchiveAccessor",
            &realm_query,
        )
        .await
        .unwrap();
        assert_eq!(moniker, Moniker::parse_str("example/component").unwrap());
        assert_eq!(protocol, "fuchsia.diagnostics.ArchiveAccessor");

        let (moniker, protocol) =
            fuzzy_search_accessors("example/component:ArchiveAccessor", &realm_query)
                .await
                .unwrap();
        assert_eq!(moniker, Moniker::parse_str("example/component").unwrap());
        assert_eq!(protocol, "fuchsia.diagnostics.ArchiveAccessor");
    }

    #[fuchsia::test]
    async fn test_fuzzy_search_accessors_disambiguation_by_exact_protocol() {
        let fake_realm_query = Rc::new(MockRealmQuery::default());
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        // MockRealmQuery::default() uses `MockRealmQueryBuilder::prefilled()` as the source of
        // truth, which sets up the following monikers and protocols:
        // - `example/component`: `fuchsia.diagnostics.ArchiveAccessor`
        // - `other/component`: `fuchsia.io.SomeOtherThing`, `fuchsia.io.MagicStuff`
        // - `foo/component`: `fuchsia.diagnostics.ArchiveAccessor.feedback`
        // - `foo/bar/thing:instance`: `fuchsia.diagnostics.ArchiveAccessor.feedback`
        //
        // Both "example/component" and "foo/component" match the moniker query "component",
        // but only "example/component" exposes the exact protocol "fuchsia.diagnostics.ArchiveAccessor"
        // ("foo/component" exposes "fuchsia.diagnostics.ArchiveAccessor.feedback").
        let (moniker, protocol) =
            fuzzy_search_accessors("component:fuchsia.diagnostics.ArchiveAccessor", &realm_query)
                .await
                .unwrap();
        assert_eq!(moniker, Moniker::parse_str("example/component").unwrap());
        assert_eq!(protocol, "fuchsia.diagnostics.ArchiveAccessor");
    }

    #[fuchsia::test]
    async fn test_fuzzy_search_accessors_disambiguation_by_both_exact_moniker_and_protocol() {
        let fake_realm_query = Rc::new(
            MockRealmQueryBuilder::new()
                .when("example/component")
                .moniker("./example/component")
                .accessors(&[
                    "fuchsia.diagnostics.ArchiveAccessor",
                    "fuchsia.diagnostics.ArchiveAccessor.feedback",
                ])
                .add()
                .when("example/component/subcomponent")
                .moniker("./example/component/subcomponent")
                .accessors(&[
                    "fuchsia.diagnostics.ArchiveAccessor",
                    "fuchsia.diagnostics.ArchiveAccessor.feedback",
                ])
                .add()
                .build(),
        );
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        let (moniker, protocol) = fuzzy_search_accessors(
            "example/component:fuchsia.diagnostics.ArchiveAccessor",
            &realm_query,
        )
        .await
        .unwrap();
        assert_eq!(moniker, Moniker::parse_str("example/component").unwrap());
        assert_eq!(protocol, "fuchsia.diagnostics.ArchiveAccessor");
    }

    #[fuchsia::test]
    async fn test_fuzzy_search_accessors_no_protocol_single_accessor() {
        let fake_realm_query = Rc::new(MockRealmQuery::default());
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        let (moniker, protocol) =
            fuzzy_search_accessors("example/component", &realm_query).await.unwrap();
        assert_eq!(moniker, Moniker::parse_str("example/component").unwrap());
        assert_eq!(protocol, "fuchsia.diagnostics.ArchiveAccessor");
    }

    #[fuchsia::test]
    async fn test_fuzzy_search_accessors_no_protocol_multiple_accessors() {
        let fake_realm_query = Rc::new(
            MockRealmQueryBuilder::new()
                .when("example/component")
                .moniker("./example/component")
                .accessors(&[
                    "fuchsia.diagnostics.ArchiveAccessor",
                    "fuchsia.diagnostics.ArchiveAccessor.feedback",
                ])
                .add()
                .build(),
        );
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        let res = fuzzy_search_accessors("example/component", &realm_query).await;
        assert_matches!(
            res,
            Err(Error::FuzzyMatchTooManyMatches(matches)) if matches.0 == [
                "example/component:fuchsia.diagnostics.ArchiveAccessor",
                "example/component:fuchsia.diagnostics.ArchiveAccessor.feedback",
            ]
        );
    }

    #[fuchsia::test]
    async fn test_fuzzy_search_accessors_moniker_not_found() {
        let fake_realm_query = Rc::new(MockRealmQuery::default());
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        let res = fuzzy_search_accessors("nonexistent:ArchiveAccessor", &realm_query).await;
        assert_matches!(res, Err(Error::SearchParameterNotFound(query)) if query == "nonexistent:ArchiveAccessor");
    }

    #[fuchsia::test]
    async fn test_fuzzy_search_accessors_protocol_not_found() {
        let fake_realm_query = Rc::new(MockRealmQuery::default());
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        let res = fuzzy_search_accessors("example:nonexistent", &realm_query).await;
        assert_matches!(res, Err(Error::SearchParameterNotFound(query)) if query == "example:nonexistent");
    }

    #[fuchsia::test]
    async fn test_fuzzy_search_accessors_too_many_matches_fuzzy_protocol() {
        let fake_realm_query = Rc::new(MockRealmQuery::default());
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        let res = fuzzy_search_accessors("foo:feedback", &realm_query).await;
        assert_matches!(
            res,
            Err(Error::FuzzyMatchTooManyMatches(matches)) if matches.0 == [
                "foo/bar/thing:instance:fuchsia.diagnostics.ArchiveAccessor.feedback",
                "foo/component:fuchsia.diagnostics.ArchiveAccessor.feedback",
            ]
        );
    }

    #[fuchsia::test]
    async fn test_fuzzy_search_accessors_too_many_matches_exact_protocol() {
        let fake_realm_query = Rc::new(MockRealmQuery::default());
        let realm_query = Rc::clone(&fake_realm_query).get_proxy().await;

        let res = fuzzy_search_accessors(
            "foo:fuchsia.diagnostics.ArchiveAccessor.feedback",
            &realm_query,
        )
        .await;
        assert_matches!(
            res,
            Err(Error::FuzzyMatchTooManyMatches(matches)) if matches.0 == [
                "foo/bar/thing:instance:fuchsia.diagnostics.ArchiveAccessor.feedback",
                "foo/component:fuchsia.diagnostics.ArchiveAccessor.feedback",
            ]
        );
    }
}
