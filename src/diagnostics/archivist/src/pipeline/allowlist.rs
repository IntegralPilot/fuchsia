// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::Error;
use anyhow::anyhow;
use diagnostics_hierarchy::HierarchyMatcher;
use fidl_fuchsia_diagnostics::{Selector, TreeNames};
use fidl_fuchsia_inspect::DEFAULT_TREE_NAME;
use moniker::ExtendedMoniker;
use selectors::SelectorExt;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
enum ComponentAllowlistState {
    // Mapping of Inspect tree names to HierarchMatchers.
    FilteringEnabled {
        names_allowlist: Arc<HashMap<String, HierarchyMatcher>>,
        all_allowlist: Option<Arc<HierarchyMatcher>>,
    },

    // Indicates that the privacy pipeline is enabled, and NO data for this component is
    // approved for exfiltration by Archivist.
    AllFilteredOut,

    // No privacy pipeline is enabled.
    FilteringDisabled,
}

pub enum PrivacyExplicitOption<T> {
    // Privacy pipeline is enabled and `Self` holds a `T`.
    Found(T),

    // Privacy pipeline is enabled and no data is held in `Self`.
    NotFound,

    // Privacy pipeline is disabled.
    FilteringDisabled,
}

#[derive(Debug, Clone)]
pub struct ComponentAllowlist(ComponentAllowlistState);

impl ComponentAllowlist {
    pub fn all_filtered_out(&self) -> bool {
        matches!(self.0, ComponentAllowlistState::AllFilteredOut)
    }

    pub fn matcher(&self, name: &str) -> PrivacyExplicitOption<&HierarchyMatcher> {
        match &self.0 {
            ComponentAllowlistState::FilteringEnabled { names_allowlist, all_allowlist } => {
                if let Some(matcher) = names_allowlist.get(name) {
                    PrivacyExplicitOption::Found(matcher)
                } else if let Some(matcher) = all_allowlist {
                    PrivacyExplicitOption::Found(matcher.as_ref())
                } else {
                    PrivacyExplicitOption::NotFound
                }
            }
            ComponentAllowlistState::AllFilteredOut => PrivacyExplicitOption::NotFound,
            ComponentAllowlistState::FilteringDisabled => PrivacyExplicitOption::FilteringDisabled,
        }
    }

    fn new<'a>(
        selectors_for_this_component: impl Iterator<Item = Result<&'a Selector, anyhow::Error>>,
    ) -> Result<Self, Error> {
        let buckets = bucketize_selectors_by_name(selectors_for_this_component)?;
        Ok(Self(ComponentAllowlistState::FilteringEnabled {
            names_allowlist: Arc::new(HashMap::from_iter(
                buckets
                    .names
                    .into_iter()
                    .map(|(k, v)| Ok((k, HierarchyMatcher::try_from(v)?)))
                    .collect::<Result<Vec<_>, Error>>()?,
            )),
            all_allowlist: buckets.all.map(HierarchyMatcher::try_from).transpose()?.map(Arc::new),
        }))
    }
}

#[cfg(test)]
impl ComponentAllowlist {
    pub fn filtering_enabled(&self) -> bool {
        match self.0 {
            ComponentAllowlistState::FilteringEnabled { .. }
            | ComponentAllowlistState::AllFilteredOut => true,
            ComponentAllowlistState::FilteringDisabled => false,
        }
    }

    pub fn new_disabled() -> Self {
        ComponentAllowlist(ComponentAllowlistState::FilteringDisabled)
    }
}

struct BucketedSelectors<'a> {
    all: Option<Vec<&'a Selector>>,
    names: HashMap<String, Vec<&'a Selector>>,
}

fn bucketize_selectors_by_name<'a>(
    selectors: impl Iterator<Item = Result<&'a Selector, anyhow::Error>>,
) -> Result<BucketedSelectors<'a>, Error> {
    let mut names_to_selectors: HashMap<_, Vec<_>> = HashMap::new();
    let mut selectors_against_all = vec![];
    // de-duplicating selectors would minimize this interim map that is returned, but
    // during construction the final HierarchyMatcher will de-duplicate, so it seems not
    // worth it here
    for s in selectors {
        let s = s.map_err(Error::Selectors)?;
        match s.tree_names {
            Some(TreeNames::Some(ref tree_names)) => {
                for name in tree_names {
                    if let Some(mapped_selectors) = names_to_selectors.get_mut(name) {
                        mapped_selectors.push(s);
                    } else {
                        names_to_selectors.insert(name.to_string(), vec![s]);
                    }
                }
            }
            None => {
                if let Some(mapped_selectors) = names_to_selectors.get_mut(DEFAULT_TREE_NAME) {
                    mapped_selectors.push(s);
                } else {
                    names_to_selectors.insert(DEFAULT_TREE_NAME.to_string(), vec![s]);
                }
            }
            Some(TreeNames::All(_)) => {
                selectors_against_all.push(s);
            }
            Some(TreeNames::__SourceBreaking { unknown_ordinal }) => {
                return Err(Error::Selectors(anyhow!(
                    "unknown TreeNames variant {unknown_ordinal} in {s:?}"
                )));
            }
        }
    }

    if selectors_against_all.is_empty() {
        return Ok(BucketedSelectors { all: None, names: names_to_selectors });
    }

    for names in names_to_selectors.values_mut() {
        names.extend(selectors_against_all.iter());
    }

    Ok(BucketedSelectors { all: Some(selectors_against_all), names: names_to_selectors })
}

#[derive(Clone)]
enum StaticHierarchyAllowlistState {
    // Mapping of components to exfiltration allowlists for the component.
    FilteringEnabled {
        component_allowlists: HashMap<ExtendedMoniker, ComponentAllowlist>,
        all_selectors: Vec<Selector>,
    },

    // The privacy pipeline is disabled.
    FilteringDisabled,
}

#[derive(Clone)]
pub struct StaticHierarchyAllowlist(StaticHierarchyAllowlistState);

impl StaticHierarchyAllowlist {
    /// Get the allowlist for the given moniker.
    ///
    /// The component must be added via `Self::add_component` before attempting
    /// to access it via this method.
    pub fn get(&self, moniker: &ExtendedMoniker) -> ComponentAllowlist {
        match &self.0 {
            StaticHierarchyAllowlistState::FilteringEnabled { component_allowlists, .. } => {
                component_allowlists
                    .get(moniker)
                    .cloned()
                    .unwrap_or(ComponentAllowlist(ComponentAllowlistState::AllFilteredOut))
            }
            StaticHierarchyAllowlistState::FilteringDisabled => {
                ComponentAllowlist(ComponentAllowlistState::FilteringDisabled)
            }
        }
    }

    pub fn new(all_selectors: Option<Vec<Selector>>) -> Self {
        if let Some(all_selectors) = all_selectors {
            return Self(StaticHierarchyAllowlistState::FilteringEnabled {
                // lazily generate the allowlists as components are added
                component_allowlists: HashMap::new(),
                all_selectors,
            });
        }

        Self(StaticHierarchyAllowlistState::FilteringDisabled)
    }

    pub const fn new_disabled() -> Self {
        Self(StaticHierarchyAllowlistState::FilteringDisabled)
    }

    pub fn remove_component(&mut self, moniker: &ExtendedMoniker) {
        match &mut self.0 {
            StaticHierarchyAllowlistState::FilteringEnabled { component_allowlists, .. } => {
                component_allowlists.remove(moniker);
            }
            StaticHierarchyAllowlistState::FilteringDisabled => {}
        }
    }

    /// Populate the allowlist for the component referred to by `moniker`.
    pub fn add_component(&mut self, moniker: ExtendedMoniker) -> Result<(), Error> {
        match &mut self.0 {
            StaticHierarchyAllowlistState::FilteringEnabled {
                all_selectors,
                component_allowlists,
            } => {
                let mut matched_selectors =
                    moniker.match_against_selectors(all_selectors.iter()).peekable();
                if matched_selectors.peek().is_none() {
                    drop(matched_selectors);
                    component_allowlists.insert(
                        moniker,
                        ComponentAllowlist(ComponentAllowlistState::AllFilteredOut),
                    );
                } else {
                    let allowlist = ComponentAllowlist::new(matched_selectors)?;
                    component_allowlists.insert(moniker, allowlist);
                }
            }
            StaticHierarchyAllowlistState::FilteringDisabled => {}
        }

        Ok(())
    }
}

#[cfg(test)]
impl StaticHierarchyAllowlist {
    pub fn filtering_enabled(&self) -> bool {
        match self.0 {
            StaticHierarchyAllowlistState::FilteringEnabled { .. } => true,
            StaticHierarchyAllowlistState::FilteringDisabled => false,
        }
    }

    pub fn component_was_added(&self, moniker: &ExtendedMoniker) -> bool {
        match &self.0 {
            StaticHierarchyAllowlistState::FilteringEnabled { component_allowlists, .. } => {
                component_allowlists.get(moniker).is_some()
            }
            StaticHierarchyAllowlistState::FilteringDisabled => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use selectors::{VerboseError, parse_selector};

    fn make_selectors(s: &[Selector]) -> impl Iterator<Item = Result<&Selector, anyhow::Error>> {
        s.iter().map(Ok)
    }

    fn matcher_found(m: PrivacyExplicitOption<&HierarchyMatcher>) -> bool {
        matches!(m, PrivacyExplicitOption::Found(_))
    }

    fn matcher_not_found(m: PrivacyExplicitOption<&HierarchyMatcher>) -> bool {
        matches!(m, PrivacyExplicitOption::NotFound)
    }

    #[fuchsia::test]
    fn component_allowlist() {
        let selectors = &[
            parse_selector::<VerboseError>(r#"*:[name=foo]root:hello"#).unwrap(),
            parse_selector::<VerboseError>(r#"*:[name=bar]root:hello"#).unwrap(),
            parse_selector::<VerboseError>(r#"*:[...]root:good"#).unwrap(),
        ];

        let selectors = make_selectors(selectors);

        let list = ComponentAllowlist::new(selectors).unwrap();
        assert!(list.filtering_enabled());

        assert!(matcher_found(list.matcher("foo")));
        assert!(matcher_found(list.matcher("bar")));
        assert!(matcher_found(list.matcher("should match all")));
    }

    #[fuchsia::test]
    fn test_bucketize_selectors() {
        let orig_selectors = &[
            parse_selector::<VerboseError>(r#"*:[name=foo]root:hello"#).unwrap(),
            parse_selector::<VerboseError>(r#"*:[name=bar]root:hello"#).unwrap(),
            parse_selector::<VerboseError>(r#"*:[...]root:something"#).unwrap(),
            parse_selector::<VerboseError>(r#"*:[name=foo]root:goodbye"#).unwrap(),
        ];

        let selectors = make_selectors(orig_selectors);

        let buckets = bucketize_selectors_by_name(selectors).unwrap();

        let named_expected = HashMap::from([
            ("foo".into(), vec![&orig_selectors[0], &orig_selectors[3], &orig_selectors[2]]),
            ("bar".into(), vec![&orig_selectors[1], &orig_selectors[2]]),
        ]);

        let all_expected = Some(vec![&orig_selectors[2]]);

        assert_eq!(buckets.names, named_expected);
        assert_eq!(buckets.all, all_expected);
    }

    #[fuchsia::test]
    fn static_hierarchy_allowlist() {
        let selectors = vec![
            parse_selector::<VerboseError>(r#"component1:[name=foo]root:foo_one"#).unwrap(),
            parse_selector::<VerboseError>(r#"component1:[...]root:all_one"#).unwrap(),
            parse_selector::<VerboseError>(r#"*:[name=foo, name=bar]root"#).unwrap(),
            parse_selector::<VerboseError>(r#"component2:[name=bar]root:bar_two"#).unwrap(),
            parse_selector::<VerboseError>(r#"component2:[name=qux]root:bar_two"#).unwrap(),
        ];

        let mut allowlist = StaticHierarchyAllowlist::new(Some(selectors));

        assert!(allowlist.filtering_enabled());

        let component_moniker = ExtendedMoniker::try_from("component1").unwrap();
        allowlist.add_component(component_moniker.clone()).unwrap();

        let list = allowlist.get(&component_moniker);

        assert!(list.filtering_enabled());

        assert!(matcher_found(list.matcher("foo")));
        assert!(matcher_found(list.matcher("bar")));
        assert!(matcher_found(list.matcher("should match all")));

        let component_moniker = ExtendedMoniker::try_from("component2").unwrap();
        allowlist.add_component(component_moniker.clone()).unwrap();

        let list = allowlist.get(&component_moniker);
        assert!(matcher_found(list.matcher("foo")));
        assert!(matcher_found(list.matcher("bar")));
        assert!(matcher_found(list.matcher("qux")));
        assert!(matcher_not_found(list.matcher("no 'all' for 2")));
    }
}
