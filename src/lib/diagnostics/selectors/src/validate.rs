// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::{StringPatternError, ValidationError};
use crate::ir;
use fidl_fuchsia_diagnostics_common as fdiagnostics;

// NOTE: if we could use the negative_impls unstable feature, we could have a single ValidateExt
// trait instead of one for each type of selector we need.
pub trait ValidateExt {
    fn validate(&self) -> Result<(), ValidationError>;
}

pub trait ValidateComponentSelectorExt {
    fn validate(&self) -> Result<(), ValidationError>;
}

pub trait ValidateTreeSelectorExt {
    fn validate(&self) -> Result<(), ValidationError>;
}

pub trait Selector {
    type Component: ComponentSelector;
    type Tree: TreeSelector;

    fn component(&self) -> Option<&Self::Component>;
    fn tree(&self) -> Option<&Self::Tree>;
}

pub trait ComponentSelector {
    type Segment: StringSelector;
    fn segments(&self) -> Option<&[Self::Segment]>;
}

pub trait TreeSelector {
    type Segment: StringSelector;

    fn node_path(&self) -> Option<&[Self::Segment]>;
    fn property(&self) -> Option<&Self::Segment>;
}

pub trait StringSelector {
    fn exact_match(&self) -> Option<&str>;
    fn pattern(&self) -> Option<&str>;
}

impl<T: Selector> ValidateExt for T {
    fn validate(&self) -> Result<(), ValidationError> {
        match (self.component(), self.tree()) {
            (Some(component), Some(tree)) => {
                component.validate()?;
                tree.validate()?;
            }
            (None, _) => return Err(ValidationError::MissingComponentSelector),
            (_, None) => return Err(ValidationError::MissingTreeSelector),
        }
        Ok(())
    }
}

impl<T: ComponentSelector> ValidateComponentSelectorExt for T {
    fn validate(&self) -> Result<(), ValidationError> {
        let segments = self.segments().unwrap_or(&[]);
        if segments.is_empty() {
            return Err(ValidationError::EmptyComponentSelector);
        }
        let last_idx = segments.len() - 1;
        for segment in &segments[..last_idx] {
            segment.validate(StringSelectorValidationOpts {
                allow_empty: false,
                ..Default::default()
            })?;
        }
        segments[last_idx].validate(StringSelectorValidationOpts {
            allow_recursive_glob: true,
            ..Default::default()
        })?;
        Ok(())
    }
}

impl<T: TreeSelector> ValidateTreeSelectorExt for T {
    fn validate(&self) -> Result<(), ValidationError> {
        let node_path = self.node_path().unwrap_or(&[]);
        if node_path.is_empty() {
            return Err(ValidationError::EmptySubtreeSelector);
        }
        for segment in node_path {
            segment.validate(StringSelectorValidationOpts::default())?;
        }
        if let Some(segment) = self.property() {
            segment.validate(StringSelectorValidationOpts::default())?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct StringSelectorValidationOpts {
    allow_recursive_glob: bool,
    allow_empty: bool,
}

trait ValidateStringSelectorExt {
    fn validate(&self, opts: StringSelectorValidationOpts) -> Result<(), ValidationError>;
}

impl<T: StringSelector> ValidateStringSelectorExt for T {
    fn validate(&self, opts: StringSelectorValidationOpts) -> Result<(), ValidationError> {
        match (self.exact_match(), self.pattern()) {
            (None, None) | (Some(_), Some(_)) => Err(ValidationError::InvalidStringSelector),
            (Some(exact_match), None) => {
                if !opts.allow_empty && exact_match.is_empty() {
                    return Err(ValidationError::InvalidStringSelector);
                }
                Ok(())
            }
            (None, Some(pattern)) => {
                if opts.allow_recursive_glob && pattern == "**" {
                    Ok(())
                } else {
                    validate_pattern(pattern)
                }
            }
        }
    }
}

/// Checks if the `target` string contains the `forbidden` string without an unescaped
/// preceding backslash.
fn contains_unescaped(target: &str, forbidden: &str) -> bool {
    if forbidden.is_empty() {
        return true;
    }
    for (idx, _) in target.match_indices(forbidden) {
        let preceding = &target[..idx];
        let backslash_count = preceding.chars().rev().take_while(|&c| c == '\\').count();
        if backslash_count % 2 == 0 {
            return true;
        }
    }
    false
}

/// Checks if the payload of an already-parsed `StringSelector::Pattern` is
/// indeed valid.
/// Does not validate correctly for patterns in the selector *language*
fn validate_pattern(pattern: &str) -> Result<(), ValidationError> {
    if pattern.is_empty() {
        return Err(ValidationError::EmptyStringPattern);
    }

    let mut errors = vec![];
    if contains_unescaped(pattern, "**") {
        errors.push(StringPatternError::UnescapedGlob);
    }
    if !errors.is_empty() {
        return Err(ValidationError::InvalidStringPattern(pattern.to_string(), errors));
    }
    Ok(())
}

impl Selector for fdiagnostics::Selector {
    type Component = fdiagnostics::ComponentSelector;
    type Tree = fdiagnostics::TreeSelector;

    fn component(&self) -> Option<&Self::Component> {
        self.component_selector.as_ref()
    }

    fn tree(&self) -> Option<&Self::Tree> {
        self.tree_selector.as_ref()
    }
}

impl ComponentSelector for fdiagnostics::ComponentSelector {
    type Segment = fdiagnostics::StringSelector;

    fn segments(&self) -> Option<&[Self::Segment]> {
        self.moniker_segments.as_deref()
    }
}

impl TreeSelector for fdiagnostics::TreeSelector {
    type Segment = fdiagnostics::StringSelector;

    fn node_path(&self) -> Option<&[Self::Segment]> {
        match self {
            Self::SubtreeSelector(t) => Some(&t.node_path[..]),
            Self::PropertySelector(p) => Some(&p.node_path[..]),
            fdiagnostics::TreeSelectorUnknown!() => None,
        }
    }

    fn property(&self) -> Option<&Self::Segment> {
        match self {
            Self::SubtreeSelector(_) => None,
            Self::PropertySelector(p) => Some(&p.target_properties),
            fdiagnostics::TreeSelectorUnknown!() => None,
        }
    }
}

impl StringSelector for fdiagnostics::StringSelector {
    fn exact_match(&self) -> Option<&str> {
        match self {
            Self::ExactMatch(s) => Some(s),
            _ => None,
        }
    }

    fn pattern(&self) -> Option<&str> {
        match self {
            Self::StringPattern(s) => Some(s),
            _ => None,
        }
    }
}

impl<'a> Selector for ir::Selector<'a> {
    type Component = ir::ComponentSelector<'a>;
    type Tree = ir::TreeSelector<'a>;

    fn component(&self) -> Option<&Self::Component> {
        Some(&self.component)
    }

    fn tree(&self) -> Option<&Self::Tree> {
        Some(&self.tree)
    }
}

impl<'a> ComponentSelector for ir::ComponentSelector<'a> {
    type Segment = ir::Segment<'a>;

    fn segments(&self) -> Option<&[Self::Segment]> {
        Some(&self.segments[..])
    }
}

impl<'a> TreeSelector for ir::TreeSelector<'a> {
    type Segment = ir::Segment<'a>;

    fn node_path(&self) -> Option<&[Self::Segment]> {
        Some(&self.node)
    }

    fn property(&self) -> Option<&Self::Segment> {
        self.property.as_ref()
    }
}

impl StringSelector for ir::Segment<'_> {
    fn exact_match(&self) -> Option<&str> {
        match self {
            Self::ExactMatch(s) => Some(s),
            _ => None,
        }
    }

    fn pattern(&self) -> Option<&str> {
        match self {
            Self::Pattern(s) => Some(s),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::LazyLock;

    static SHARED_PASSING_TEST_CASES: LazyLock<Vec<(Vec<&'static str>, &'static str)>> =
        LazyLock::new(|| {
            vec![
                (vec![r#"abc"#, r#"def"#, r#"g"#], r#"bob"#),
                (vec![r#"\**"#], r#"\**"#),
                (vec![r#"\/"#], r#"\/"#),
                (vec![r#"\:"#], r#"\:"#),
                (vec![r#"asda\\\:"#], r#"a"#),
                (vec![r#"asda*"#], r#"a"#),
            ]
        });
    static SHARED_FAILING_TEST_CASES: LazyLock<Vec<(Vec<&'static str>, &'static str)>> =
        LazyLock::new(|| {
            vec![
                // Checking that path nodes ending with offlimits
                // chars are still identified.
                (vec![r#"a**"#], r#"a"#),
                // Checking that path nodes starting with offlimits
                // chars are still identified.
                (vec![r#"**a"#], r#"a"#),
                // Neither moniker segments nor node paths
                // are allowed to be empty.
                (vec![], r#"bob"#),
            ]
        });

    #[fuchsia::test]
    fn tree_selector_validator_test() {
        let unique_failing_test_cases = [
            // All failing validators due to property selectors are
            // unique since the component validator doesn't look at them.
            (vec![r#"a"#], r#"**"#),
        ];

        fn create_tree_selector(node_path: &[&str], property: &str) -> fdiagnostics::TreeSelector {
            let node_path = node_path
                .iter()
                .map(|path_node_str| {
                    fdiagnostics::StringSelector::StringPattern(path_node_str.to_string())
                })
                .collect::<Vec<fdiagnostics::StringSelector>>();
            let target_properties =
                fdiagnostics::StringSelector::StringPattern(property.to_string());
            fdiagnostics::TreeSelector::PropertySelector(fdiagnostics::PropertySelector {
                node_path,
                target_properties,
            })
        }

        for (node_path, property) in SHARED_PASSING_TEST_CASES.iter() {
            let tree_selector = create_tree_selector(node_path, property);
            assert!(tree_selector.validate().is_ok());
        }

        for (node_path, property) in SHARED_FAILING_TEST_CASES.iter() {
            let tree_selector = create_tree_selector(node_path, property);
            assert!(
                ValidateTreeSelectorExt::validate(&tree_selector).is_err(),
                "Failed to validate tree selector: {tree_selector:?}"
            );
        }

        for (node_path, property) in unique_failing_test_cases.iter() {
            let tree_selector = create_tree_selector(node_path, property);
            assert!(
                ValidateTreeSelectorExt::validate(&tree_selector).is_err(),
                "Failed to validate tree selector: {tree_selector:?}"
            );
        }
    }

    #[fuchsia::test]
    fn component_selector_validator_test() {
        fn create_component_selector(
            component_moniker: &[&str],
        ) -> fdiagnostics::ComponentSelector {
            fdiagnostics::ComponentSelector {
                moniker_segments: Some(
                    component_moniker
                        .iter()
                        .map(|path_node_str| {
                            fdiagnostics::StringSelector::StringPattern(path_node_str.to_string())
                        })
                        .collect::<Vec<fdiagnostics::StringSelector>>(),
                ),
                ..fdiagnostics::ComponentSelector::default()
            }
        }

        for (component_moniker, _) in SHARED_PASSING_TEST_CASES.iter() {
            let component_selector = create_component_selector(component_moniker);

            assert!(component_selector.validate().is_ok());
        }

        for (component_moniker, _) in SHARED_FAILING_TEST_CASES.iter() {
            let component_selector = create_component_selector(component_moniker);

            assert!(
                component_selector.validate().is_err(),
                "Failed to validate component selector: {component_selector:?}"
            );
        }
    }

    #[fuchsia::test]
    fn test_contains_unescaped() {
        assert!(contains_unescaped("foo**bar", "**"));
        assert!(!contains_unescaped("foo\\**bar", "**"));
        assert!(contains_unescaped("foo\\\\**bar", "**"));
        assert!(!contains_unescaped("foo\\\\\\**bar", "**"));
        assert!(contains_unescaped("🦀**", "**"));
        assert!(!contains_unescaped("🦀\\**", "**"));
    }
}
