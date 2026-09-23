// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_diagnostics_common as fdiagnostics;
use std::borrow::Cow;
use std::fmt::Debug;

#[derive(Debug, Eq, PartialEq)]
pub enum Segment<'a> {
    ExactMatch(Cow<'a, str>),
    Pattern(Cow<'a, str>),
}

fn contains_unescaped(s: &str, unescaped_char: char) -> bool {
    let mut iter = s.chars();
    while let Some(c) = iter.next() {
        match c {
            c2 if c2 == unescaped_char => return true,
            '\\' => {
                // skip escaped characters
                let _ = iter.next();
            }
            _ => {}
        }
    }
    false
}

impl<'a> From<&'a str> for Segment<'a> {
    fn from(s: &'a str) -> Segment<'a> {
        if contains_unescaped(s, '*') {
            return Segment::Pattern(Cow::Owned(unescape(s, &['*'])));
        }
        if !s.contains('\\') {
            return Segment::ExactMatch(Cow::from(s));
        }
        Segment::ExactMatch(Cow::Owned(unescape(s, &[])))
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum TreeNames<'a> {
    Some(Vec<Cow<'a, str>>),
    All,
}

impl<'a> From<Vec<&'a str>> for TreeNames<'a> {
    fn from(vec: Vec<&'a str>) -> TreeNames<'a> {
        let mut payload = vec![];
        for name in vec {
            if name.contains('\\') {
                payload.push(Cow::Owned(unescape(name, &[])));
            } else {
                payload.push(Cow::Borrowed(name));
            }
        }
        TreeNames::Some(payload)
    }
}

// Given a escaped string, removes all the escaped characters (`\\`) and returns a new string
// without them. It'll keep characters present in the `except` list escaped.
fn unescape(value: &str, except: &[char]) -> String {
    let mut result = String::with_capacity(value.len());
    let mut iter = value.chars();
    while let Some(c) = iter.next() {
        match c {
            '\\' => {
                // push unescaped character since we are constructing an exact match.
                if let Some(c) = iter.next() {
                    if except.contains(&c) {
                        result.push('\\')
                    }
                    result.push(c);
                }
            }
            c => result.push(c),
        }
    }
    result
}

#[derive(Debug, Eq, PartialEq)]
pub struct TreeSelector<'a> {
    pub node: Vec<Segment<'a>>,
    pub property: Option<Segment<'a>>,
    pub tree_names: Option<TreeNames<'a>>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ComponentSelector<'a> {
    pub segments: Vec<Segment<'a>>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Selector<'a> {
    pub component: ComponentSelector<'a>,
    pub tree: TreeSelector<'a>,
}

impl From<Selector<'_>> for fdiagnostics::Selector {
    fn from(mut selector: Selector<'_>) -> fdiagnostics::Selector {
        let tree_names = selector.tree.tree_names.take();
        fdiagnostics::Selector {
            component_selector: Some(selector.component.into()),
            tree_selector: Some(selector.tree.into()),
            tree_names: tree_names.map(|names| names.into()),
            ..Default::default()
        }
    }
}

impl From<ComponentSelector<'_>> for fdiagnostics::ComponentSelector {
    fn from(component_selector: ComponentSelector<'_>) -> fdiagnostics::ComponentSelector {
        fdiagnostics::ComponentSelector {
            moniker_segments: Some(
                component_selector.segments.into_iter().map(|segment| segment.into()).collect(),
            ),
            ..Default::default()
        }
    }
}

impl From<TreeSelector<'_>> for fdiagnostics::TreeSelector {
    fn from(tree_selector: TreeSelector<'_>) -> fdiagnostics::TreeSelector {
        let node_path = tree_selector.node.into_iter().map(|s| s.into()).collect();
        match tree_selector.property {
            None => fdiagnostics::TreeSelector::SubtreeSelector(fdiagnostics::SubtreeSelector {
                node_path,
            }),
            Some(property) => {
                fdiagnostics::TreeSelector::PropertySelector(fdiagnostics::PropertySelector {
                    node_path,
                    target_properties: property.into(),
                })
            }
        }
    }
}

impl From<TreeNames<'_>> for fdiagnostics::TreeNames {
    fn from(tree_names: TreeNames<'_>) -> fdiagnostics::TreeNames {
        match tree_names {
            TreeNames::All => fdiagnostics::TreeNames::All(fdiagnostics::All {}),
            TreeNames::Some(names) => {
                fdiagnostics::TreeNames::Some(names.iter().map(|n| n.to_string()).collect())
            }
        }
    }
}

impl From<Segment<'_>> for fdiagnostics::StringSelector {
    fn from(segment: Segment<'_>) -> fdiagnostics::StringSelector {
        match segment {
            Segment::ExactMatch(s) => fdiagnostics::StringSelector::ExactMatch(s.into_owned()),
            Segment::Pattern(s) => fdiagnostics::StringSelector::StringPattern(s.into_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn convert_string_to_segment() {
        assert_eq!(Segment::ExactMatch(Cow::Borrowed("abc")), "abc".into());
        assert_eq!(Segment::Pattern("a*c".into()), "a*c".into());
        assert_eq!(Segment::ExactMatch(Cow::Owned("ac*".into())), "ac\\*".into());
        assert_eq!(Segment::Pattern("a\\*c*".into()), "a\\*c*".into());
    }
}
