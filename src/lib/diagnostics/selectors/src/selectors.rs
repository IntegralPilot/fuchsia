// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::*;
use crate::parser::{self, ParsingError, RequireEscaped, VerboseError};
use crate::validate::*;
use anyhow::format_err;
use buf_read_ext::BufReadExt as _;
use fidl_fuchsia_diagnostics_common::{
    self as fdiagnostics, ComponentSelector, LogInterestSelector, PropertySelector, Selector,
    SelectorArgument, StringSelector, SubtreeSelector, TreeNames, TreeSelector,
};

use fidl_fuchsia_inspect_common::DEFAULT_TREE_NAME;
use itertools::Itertools;
use moniker::{
    BorrowedChildName, EXTENDED_MONIKER_COMPONENT_MANAGER_STR, ExtendedMoniker, Moniker,
};
use std::borrow::{Borrow, Cow};
use std::fs;
use std::io::BufReader;
use std::iter::once;
use std::path::Path;
use std::sync::Arc;

#[cfg(fuchsia_api_level_less_than = "27")]
use fidl_fuchsia_diagnostics::{Interest, Severity};
#[cfg(fuchsia_api_level_at_least = "27")]
use fidl_fuchsia_diagnostics_types::{Interest, Severity};

// Character used to delimit the different sections of an inspect selector,
// the component selector, the tree selector, and the property selector.
pub const SELECTOR_DELIMITER: char = ':';

// Character used to delimit nodes within a component hierarchy path.
const PATH_NODE_DELIMITER: char = '/';

// Character used to escape interperetation of this parser's "special
// characters"; *, /, :, and \.
pub const ESCAPE_CHARACTER: char = '\\';

const TAB_CHAR: char = '\t';
const SPACE_CHAR: char = ' ';

// Pattern used to encode wildcard.
const WILDCARD_SYMBOL_CHAR: char = '*';

const RECURSIVE_WILDCARD_SYMBOL_STR: &str = "**";

const ROOT_SEGMENT: &str = "<root>";

/// Returns true iff a component selector uses the recursive glob.
/// Assumes the selector has already been validated.
pub fn contains_recursive_glob(component_selector: &ComponentSelector) -> bool {
    // Unwrap as a valid selector must contain these fields.
    let last_segment = component_selector.moniker_segments.as_ref().unwrap().last().unwrap();
    string_selector_contains_recursive_glob(last_segment)
}

fn string_selector_contains_recursive_glob(selector: &StringSelector) -> bool {
    matches!(
        selector,
        StringSelector::StringPattern(pattern) if pattern == RECURSIVE_WILDCARD_SYMBOL_STR
    )
}

/// Extracts and validates or parses a selector from a `SelectorArgument`.
pub fn take_from_argument<E>(arg: SelectorArgument) -> Result<Selector, Error>
where
    E: for<'a> ParsingError<'a>,
{
    match arg {
        SelectorArgument::StructuredSelector(s) => {
            s.validate()?;
            Ok(s)
        }
        SelectorArgument::RawSelector(r) => parse_selector::<VerboseError>(&r),
        _ => Err(Error::InvalidSelectorArgument),
    }
}

/// Converts an unparsed tree selector string into a TreeSelector.
pub fn parse_tree_selector<'a, E>(
    unparsed_tree_selector: &'a str,
) -> Result<TreeSelector, ParseError>
where
    E: ParsingError<'a>,
{
    let result = parser::standalone_tree_selector::<E>(unparsed_tree_selector)?;
    Ok(result.into())
}

/// Converts an unparsed component selector string into a ComponentSelector.
pub fn parse_component_selector<'a, E>(
    unparsed_component_selector: &'a str,
) -> Result<ComponentSelector, ParseError>
where
    E: ParsingError<'a>,
{
    let result = parser::consuming_component_selector::<E>(
        unparsed_component_selector,
        RequireEscaped::COLONS,
    )?;
    Ok(result.into())
}

fn parse_component_selector_no_escaping<'a, E>(
    unparsed_component_selector: &'a str,
) -> Result<ComponentSelector, ParseError>
where
    E: ParsingError<'a>,
{
    let result = parser::consuming_component_selector::<E>(
        unparsed_component_selector,
        RequireEscaped::empty(),
    )?;
    Ok(result.into())
}

/// Parses a log severity selector of the form `component_selector#SEVERITY`. For example:
/// core/foo#DEBUG.
pub fn parse_log_interest_selector(selector: &str) -> Result<LogInterestSelector, anyhow::Error> {
    let default_invalid_selector_err = format_err!(
        "Invalid component interest selector: '{}'. Expecting: '/some/moniker/selector#<log-level>'.",
        selector
    );
    let mut parts = selector.split('#');

    // Split each arg into sub string vectors containing strings
    // for component [0] and interest [1] respectively.
    let Some(component) = parts.next() else {
        return Err(default_invalid_selector_err);
    };
    let Some(interest) = parts.next() else {
        return Err(format_err!(
            concat!(
                "Missing <log-level> in selector. Expecting: '{}#<log-level>', ",
                "such as #DEBUG or #INFO."
            ),
            selector
        ));
    };
    if parts.next().is_some() {
        return Err(default_invalid_selector_err);
    }
    let parsed_selector = match parse_component_selector_no_escaping::<VerboseError>(component) {
        Ok(s) => s,
        Err(e) => {
            return Err(format_err!(
                "Invalid component interest selector: '{}'. Error: {}",
                selector,
                e
            ));
        }
    };
    let Some(min_severity) = parse_severity(interest.to_uppercase().as_ref()) else {
        return Err(format_err!(
            concat!(
                "Invalid <log-level> in selector '{}'. Expecting: a min log level ",
                "such as #DEBUG or #INFO."
            ),
            selector
        ));
    };
    Ok(LogInterestSelector {
        selector: parsed_selector,
        interest: Interest { min_severity: Some(min_severity), ..Default::default() },
    })
}

/// Parses a log severity selector of the form `component_selector#SEVERITY` or just `SEVERITY`.
/// For example: `core/foo#DEBUG` or `INFO`.
pub fn parse_log_interest_selector_or_severity(
    selector: &str,
) -> Result<LogInterestSelector, anyhow::Error> {
    if let Some(min_severity) = parse_severity(selector.to_uppercase().as_ref()) {
        return Ok(LogInterestSelector {
            selector: ComponentSelector {
                moniker_segments: Some(vec![StringSelector::StringPattern("**".into())]),
                ..Default::default()
            },
            interest: Interest { min_severity: Some(min_severity), ..Default::default() },
        });
    }
    parse_log_interest_selector(selector)
}

fn parse_severity(severity: &str) -> Option<Severity> {
    match severity {
        "TRACE" => Some(Severity::Trace),
        "DEBUG" => Some(Severity::Debug),
        "INFO" => Some(Severity::Info),
        "WARN" => Some(Severity::Warn),
        "ERROR" => Some(Severity::Error),
        "FATAL" => Some(Severity::Fatal),
        _ => None,
    }
}

/// Converts an unparsed Inspect selector into a ComponentSelector and TreeSelector.
pub fn parse_selector<E>(unparsed_selector: &str) -> Result<Selector, Error>
where
    for<'a> E: ParsingError<'a>,
{
    let result = parser::selector::<E>(unparsed_selector)?;
    Ok(result.into())
}

pub fn parse_verbose(unparsed_selector: &str) -> Result<Selector, Error> {
    parse_selector::<VerboseError>(unparsed_selector)
}

/// Remove any comments process a quoted line.
pub fn parse_selector_file<E>(selector_file: &Path) -> Result<Vec<Selector>, Error>
where
    E: for<'a> ParsingError<'a>,
{
    let selector_file = fs::File::open(selector_file)?;
    let mut result = Vec::new();
    let mut reader = BufReader::new(selector_file);
    let mut lines = reader.lending_lines();
    while let Some(line) = lines.next() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        if let Some(selector) = parser::selector_or_comment::<E>(line)? {
            result.push(selector.into());
        }
    }
    Ok(result)
}

/// Helper method for converting ExactMatch StringSelectors to regex. We must
/// escape all special characters on the behalf of the selector author when converting
/// exact matches to regex.
fn is_special_character(character: char) -> bool {
    character == ESCAPE_CHARACTER
        || character == PATH_NODE_DELIMITER
        || character == SELECTOR_DELIMITER
        || character == WILDCARD_SYMBOL_CHAR
        || character == SPACE_CHAR
        || character == TAB_CHAR
}

/// Sanitizes raw strings from the system such that they align with the
/// special-character and escaping semantics of the Selector format.
///
/// Sanitization escapes the known special characters in the selector language.
pub fn sanitize_string_for_selectors(node: &str) -> Cow<'_, str> {
    if node.is_empty() {
        return Cow::Borrowed(node);
    }

    let mut token_builder = TokenBuilder::new(node);
    for (index, node_char) in node.char_indices() {
        token_builder.maybe_init(index);
        if is_special_character(node_char) {
            token_builder.turn_into_string();
            token_builder.push(ESCAPE_CHARACTER, index);
        }
        token_builder.push(node_char, index);
    }

    token_builder.take()
}

/// Sanitizes a moniker raw string such that it can be used in a selector.
/// Monikers have a restricted set of characters `a-z`, `0-9`, `_`, `.`, `-`.
/// Each moniker segment is separated by a `\`. Segments for collections also contain `:`.
/// That `:` will be escaped.
pub fn sanitize_moniker_for_selectors(moniker: impl AsRef<str>) -> String {
    moniker.as_ref().replace(":", "\\:")
}

fn match_moniker_against_component_selector<I, S>(
    mut moniker_segments: I,
    component_selector: &ComponentSelector,
) -> Result<bool, anyhow::Error>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let selector_segments = match &component_selector.moniker_segments {
        Some(path_vec) => path_vec,
        None => return Err(format_err!("Component selectors require moniker segments.")),
    };

    for (i, selector_segment) in selector_segments.iter().enumerate() {
        // If the selector is longer than the moniker, then there's no match.
        let Some(moniker_segment) = moniker_segments.next() else {
            return Ok(false);
        };

        // If we are in the last segment and we find a recursive glob, then it's a match.
        if i == selector_segments.len() - 1
            && string_selector_contains_recursive_glob(selector_segment)
        {
            return Ok(true);
        }

        if !match_string(selector_segment, moniker_segment.as_ref()) {
            return Ok(false);
        }
    }

    // We must have consumed all moniker segments.
    Ok(moniker_segments.next().is_none())
}

/// Checks whether or not a given selector matches a given moniker and if the given `tree_name` is
/// present in the selector's tree-name-filter list.
///
/// Accounts for semantics like unspecified tree-name-filter lists.
///
/// Returns an error if the selector is invalid.
fn match_component_and_tree_name<T>(
    moniker: impl AsRef<[T]>,
    tree_name: &str,
    selector: &Selector,
) -> Result<bool, anyhow::Error>
where
    T: AsRef<str>,
{
    Ok(match_component_moniker_against_selector(moniker, selector)?
        && match_tree_name_against_selector(tree_name, selector))
}

/// Checks whether or not a given `tree_name` is present in the selector's
/// tree-name-filter list.
///
/// Accounts for semantics like unspecified tree-name-filter lists.
pub fn match_tree_name_against_selector(tree_name: &str, selector: &Selector) -> bool {
    match selector.tree_names.as_ref() {
        Some(TreeNames::All(_)) => true,

        Some(TreeNames::Some(filters)) => filters.iter().any(|f| f == tree_name),

        None => tree_name == DEFAULT_TREE_NAME,

        Some(TreeNames::__SourceBreaking { .. }) => false,
    }
}

/// Evaluates a component moniker against a single selector, returning
/// True if the selector matches the component, else false.
///
/// Requires: hierarchy_path is not empty.
///           selectors contains valid Selectors.
fn match_component_moniker_against_selector<T>(
    moniker: impl AsRef<[T]>,
    selector: &Selector,
) -> Result<bool, anyhow::Error>
where
    T: AsRef<str>,
{
    selector.validate()?;

    if moniker.as_ref().is_empty() {
        return Err(format_err!(
            "Cannot have empty monikers, at least the component name is required."
        ));
    }

    // Unwrap is safe because the validator ensures there is a component selector.
    let component_selector = selector.component_selector.as_ref().unwrap();

    match_moniker_against_component_selector(moniker.as_ref().iter(), component_selector)
}

/// Evaluates a component moniker against a list of selectors, returning
/// all of the selectors which are matches for that moniker.
///
/// Requires: hierarchy_path is not empty.
///           selectors contains valid Selectors.
fn match_component_moniker_against_selectors<'a>(
    moniker: Vec<String>,
    selectors: impl IntoIterator<Item = &'a Selector>,
) -> impl Iterator<Item = Result<&'a Selector, anyhow::Error>> {
    selectors
        .into_iter()
        .map(|selector| {
            selector.validate()?;
            Ok(selector)
        })
        .filter_map(move |selector| -> Option<Result<&'a Selector, anyhow::Error>> {
            let Ok(selector) = selector else {
                return Some(selector);
            };
            match_component_moniker_against_selector(moniker.as_slice(), selector)
                .map(|is_match| if is_match { Some(selector) } else { None })
                .transpose()
        })
}

/// Evaluates a component moniker against a list of component selectors, returning
/// all of the component selectors which are matches for that moniker.
///
/// Requires: moniker is not empty.
///           component_selectors contains valid ComponentSelectors.
fn match_moniker_against_component_selectors<'a, S, T>(
    moniker: &[T],
    selectors: &'a [S],
) -> Result<Vec<&'a ComponentSelector>, anyhow::Error>
where
    S: Borrow<ComponentSelector> + 'a,
    T: AsRef<str> + std::string::ToString,
{
    if moniker.is_empty() {
        return Err(format_err!(
            "Cannot have empty monikers, at least the component name is required."
        ));
    }

    let component_selectors = selectors
        .iter()
        .map(|selector| {
            let component_selector = selector.borrow();
            component_selector.validate()?;
            Ok(component_selector)
        })
        .collect::<Result<Vec<&ComponentSelector>, anyhow::Error>>();

    component_selectors?
        .iter()
        .filter_map(|selector| {
            match_moniker_against_component_selector(moniker.iter(), selector)
                .map(|is_match| if is_match { Some(*selector) } else { None })
                .transpose()
        })
        .collect::<Result<Vec<&ComponentSelector>, anyhow::Error>>()
}

/// Returns true if `moniker` matches one of `selectors`.
///
/// # Panics
///
/// This can panic if any selector in `selectors` is invalid; it assumes `selectors` is valid.
pub fn matches_selectors(moniker: &ExtendedMoniker, selectors: &[ComponentSelector]) -> bool {
    match moniker {
        ExtendedMoniker::ComponentManager => selectors.iter().any(|s| {
            match_moniker_against_component_selector(
                once(EXTENDED_MONIKER_COMPONENT_MANAGER_STR),
                s,
            )
            .unwrap()
        }),
        ExtendedMoniker::ComponentInstance(moniker) => {
            if moniker.is_root() {
                selectors.iter().any(|s| {
                    match_moniker_against_component_selector(once(ROOT_SEGMENT), s).unwrap()
                })
            } else {
                selectors.iter().any(|s| {
                    match_moniker_against_component_selector(moniker.into_iter(), s).unwrap()
                })
            }
        }
    }
}

/// Settings for how to construct a displayable string from a
/// `fidl_fuchsia_diagnostics::Selector`.
pub struct SelectorDisplayOptions {
    allow_wrapper_quotes: bool,
}

impl std::default::Default for SelectorDisplayOptions {
    fn default() -> Self {
        Self { allow_wrapper_quotes: true }
    }
}

impl SelectorDisplayOptions {
    /// Causes a selector to never be wrapped in exterior quotes.
    pub fn never_wrap_in_quotes() -> Self {
        Self { allow_wrapper_quotes: false }
    }
}

/// Format a |Selector| as a string.
///
/// Returns the formatted |Selector|, or an error if the |Selector| is invalid.
///
/// Note that the output will always include both a component and tree selector. If your input is
/// simply "moniker" you will likely see "moniker:root" as many clients implicitly append "root" if
/// it is not present (e.g. iquery).
///
/// Name filter lists will only be shown if they have non-default tree names.
pub fn selector_to_string(
    selector: &Selector,
    opts: SelectorDisplayOptions,
) -> Result<String, anyhow::Error> {
    fn contains_chars_requiring_wrapper_quotes(segment: &str) -> bool {
        segment.contains('/') || segment.contains('*')
    }

    selector.validate()?;

    let component_selector = selector
        .component_selector
        .as_ref()
        .ok_or_else(|| format_err!("component selector missing"))?;
    let (node_path, maybe_property_selector) = match selector
        .tree_selector
        .as_ref()
        .ok_or_else(|| format_err!("tree selector missing"))?
    {
        TreeSelector::SubtreeSelector(SubtreeSelector { node_path, .. }) => (node_path, None),
        TreeSelector::PropertySelector(PropertySelector {
            node_path, target_properties, ..
        }) => (node_path, Some(target_properties)),
        _ => return Err(format_err!("unknown tree selector type")),
    };

    let mut needs_to_be_quoted = false;
    let result = component_selector
        .moniker_segments
        .as_ref()
        .ok_or_else(|| format_err!("moniker segments missing in component selector"))?
        .iter()
        .map(|segment| match segment {
            StringSelector::StringPattern(p) => {
                needs_to_be_quoted = true;
                Ok(p)
            }
            StringSelector::ExactMatch(s) => {
                needs_to_be_quoted |= contains_chars_requiring_wrapper_quotes(s);
                Ok(s)
            }
            fdiagnostics::StringSelectorUnknown!() => {
                Err(format_err!("uknown StringSelector variant"))
            }
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .join("/");

    let mut result = sanitize_moniker_for_selectors(&result);

    let mut tree_selector_str = node_path
        .iter()
        .map(|segment| {
            Ok(match segment {
                StringSelector::StringPattern(p) => {
                    needs_to_be_quoted = true;
                    p.to_string()
                }
                StringSelector::ExactMatch(s) => {
                    needs_to_be_quoted |= contains_chars_requiring_wrapper_quotes(s);
                    sanitize_string_for_selectors(s).to_string()
                }
                fdiagnostics::StringSelectorUnknown!() => {
                    return Err(format_err!("uknown StringSelector variant"));
                }
            })
        })
        .collect::<Result<Vec<String>, _>>()?
        .into_iter()
        .join("/");

    if let Some(target_property) = maybe_property_selector {
        tree_selector_str.push(':');
        tree_selector_str.push_str(&match target_property {
            StringSelector::StringPattern(p) => {
                needs_to_be_quoted = true;
                p.to_string()
            }
            StringSelector::ExactMatch(s) => {
                needs_to_be_quoted |= contains_chars_requiring_wrapper_quotes(s);
                sanitize_string_for_selectors(s).to_string()
            }
            fdiagnostics::StringSelectorUnknown!() => {
                return Err(format_err!("uknown StringSelector variant"));
            }
        });
    }

    tree_selector_str = match &selector.tree_names {
        None => tree_selector_str,
        Some(names) => match names {
            TreeNames::Some(names) => {
                let list = names
                    .iter()
                    .filter_map(|name| {
                        if name == DEFAULT_TREE_NAME {
                            return None;
                        }

                        for c in name.chars() {
                            if !(c.is_alphanumeric() || c == '-' || c == '_') {
                                return Some(format!(r#"name="{name}""#));
                            }
                        }

                        Some(format!("name={name}"))
                    })
                    .join(",");

                if list.is_empty() {
                    tree_selector_str
                } else {
                    needs_to_be_quoted = true;
                    format!("[{list}]{tree_selector_str}")
                }
            }
            TreeNames::All(_) => {
                needs_to_be_quoted = true;
                format!("[...]{tree_selector_str}")
            }
            fdiagnostics::TreeNamesUnknown!() => {
                return Err(format_err!("unknown TreeNames variant"));
            }
        },
    };

    result.push_str(&format!(":{tree_selector_str}"));

    if needs_to_be_quoted && opts.allow_wrapper_quotes {
        Ok(format!(r#""{result}""#))
    } else {
        Ok(result)
    }
}

/// Match a selector against a target string.
pub fn match_string(selector: &StringSelector, target: impl AsRef<str>) -> bool {
    match selector {
        StringSelector::ExactMatch(s) => s == target.as_ref(),
        StringSelector::StringPattern(pattern) => match_pattern(pattern, target.as_ref()),
        _ => false,
    }
}

fn match_pattern(pattern: &str, target: &str) -> bool {
    // Tokenize the string. From: "a*bc*d" to "a, bc, d".
    let mut pattern_tokens = vec![];
    let mut token = TokenBuilder::new(pattern);
    let mut chars = pattern.char_indices();

    while let Some((index, curr_char)) = chars.next() {
        token.maybe_init(index);

        // If we find a backslash then push the next character directly to our new string.
        match curr_char {
            '\\' => {
                match chars.next() {
                    Some((i, c)) => {
                        token.turn_into_string();
                        token.push(c, i);
                    }
                    // We found a backslash without a character to its right. Return false as this
                    // isn't valid.
                    None => return false,
                }
            }
            '*' => {
                if !token.is_empty() {
                    pattern_tokens.push(token.take());
                }
                token = TokenBuilder::new(pattern);
            }
            c => {
                token.push(c, index);
            }
        }
    }

    // Push the remaining token if there's any.
    if !token.is_empty() {
        pattern_tokens.push(token.take());
    }

    // Exit early. We only have *'s.
    if pattern_tokens.is_empty() && !pattern.is_empty() {
        return true;
    }

    // If the pattern doesn't begin with a * and the target string doesn't start with the first
    // pattern token, we can exit.
    if !pattern.starts_with('*') && !target.starts_with(pattern_tokens[0].as_ref()) {
        return false;
    }

    // If the last character of the pattern is not an unescaped * and the target string doesn't end
    // with the last token in the pattern, then we can exit.
    if !pattern.ends_with('*')
        && pattern.chars().rev().nth(1) != Some('\\')
        && !target.ends_with(pattern_tokens[pattern_tokens.len() - 1].as_ref())
    {
        return false;
    }

    // We must find all pattern tokens in the target string in order. If we don't find one then we
    // fail.
    let mut cur_string = target;
    for pattern in pattern_tokens.iter() {
        match cur_string.find(pattern.as_ref()) {
            Some(i) => {
                cur_string = &cur_string[i + pattern.len()..];
            }
            None => {
                return false;
            }
        }
    }

    true
}

// Utility to allow matching the string cloning only when necessary, this is when we run into a
// escaped character.
#[derive(Debug)]
enum TokenBuilder<'a> {
    Init(&'a str),
    Slice { string: &'a str, start: usize, end: usize },
    String(String),
}

impl<'a> TokenBuilder<'a> {
    fn new(string: &'a str) -> Self {
        Self::Init(string)
    }

    fn maybe_init(&mut self, start_index: usize) {
        let Self::Init(s) = self else {
            return;
        };
        *self = Self::Slice { string: s, start: start_index, end: start_index };
    }

    fn turn_into_string(&mut self) {
        if let Self::Slice { string, start, end } = self {
            *self = Self::String(string[*start..*end].to_string());
        }
    }

    fn push(&mut self, c: char, index: usize) {
        match self {
            Self::Slice { end, .. } => {
                *end = index + c.len_utf8();
            }
            Self::String(s) => s.push(c),
            Self::Init(_) => unreachable!(),
        }
    }

    fn take(self) -> Cow<'a, str> {
        match self {
            Self::Slice { string, start, end } => Cow::Borrowed(&string[start..end]),
            Self::String(s) => Cow::Owned(s),
            Self::Init(_) => unreachable!(),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Slice { start, end, .. } => start >= end,
            Self::String(s) => s.is_empty(),
            Self::Init(_) => true,
        }
    }
}

pub trait SelectorExt {
    fn match_against_selectors<'a>(
        &self,
        selectors: impl IntoIterator<Item = &'a Selector>,
    ) -> impl Iterator<Item = Result<&'a Selector, anyhow::Error>>;

    /// Invalid selectors are filtered out.
    fn match_against_selectors_and_tree_name<'a>(
        &self,
        tree_name: &str,
        selectors: impl IntoIterator<Item = &'a Selector>,
    ) -> impl Iterator<Item = &'a Selector>;

    fn match_against_component_selectors<'a, S>(
        &self,
        selectors: &'a [S],
    ) -> Result<Vec<&'a ComponentSelector>, anyhow::Error>
    where
        S: Borrow<ComponentSelector>;

    fn into_component_selector(self) -> ComponentSelector;

    fn matches_selector(&self, selector: &Selector) -> Result<bool, anyhow::Error>;

    fn matches_component_selector(
        &self,
        selector: &ComponentSelector,
    ) -> Result<bool, anyhow::Error>;

    fn sanitized(&self) -> String;
}

impl SelectorExt for ExtendedMoniker {
    fn match_against_selectors<'a>(
        &self,
        selectors: impl IntoIterator<Item = &'a Selector>,
    ) -> impl Iterator<Item = Result<&'a Selector, anyhow::Error>> {
        let s = match self {
            ExtendedMoniker::ComponentManager => {
                vec![EXTENDED_MONIKER_COMPONENT_MANAGER_STR.to_string()]
            }
            ExtendedMoniker::ComponentInstance(moniker) => {
                SegmentIterator::from(moniker).collect::<Vec<_>>()
            }
        };

        match_component_moniker_against_selectors(s, selectors)
    }

    fn match_against_selectors_and_tree_name<'a>(
        &self,
        tree_name: &str,
        selectors: impl IntoIterator<Item = &'a Selector>,
    ) -> impl Iterator<Item = &'a Selector> {
        let m = match self {
            ExtendedMoniker::ComponentManager => {
                vec![EXTENDED_MONIKER_COMPONENT_MANAGER_STR.to_string()]
            }
            ExtendedMoniker::ComponentInstance(moniker) => {
                SegmentIterator::from(moniker).collect::<Vec<_>>()
            }
        };

        selectors
            .into_iter()
            .filter(move |s| match_component_and_tree_name(&m, tree_name, s).unwrap_or(false))
    }

    fn match_against_component_selectors<'a, S>(
        &self,
        selectors: &'a [S],
    ) -> Result<Vec<&'a ComponentSelector>, anyhow::Error>
    where
        S: Borrow<ComponentSelector>,
    {
        match self {
            ExtendedMoniker::ComponentManager => match_moniker_against_component_selectors(
                &[EXTENDED_MONIKER_COMPONENT_MANAGER_STR],
                selectors,
            ),
            ExtendedMoniker::ComponentInstance(moniker) => {
                moniker.match_against_component_selectors(selectors)
            }
        }
    }

    fn matches_selector(&self, selector: &Selector) -> Result<bool, anyhow::Error> {
        match self {
            ExtendedMoniker::ComponentManager => match_component_moniker_against_selector(
                [EXTENDED_MONIKER_COMPONENT_MANAGER_STR],
                selector,
            ),
            ExtendedMoniker::ComponentInstance(moniker) => moniker.matches_selector(selector),
        }
    }

    fn matches_component_selector(
        &self,
        selector: &ComponentSelector,
    ) -> Result<bool, anyhow::Error> {
        match self {
            ExtendedMoniker::ComponentManager => match_moniker_against_component_selector(
                [EXTENDED_MONIKER_COMPONENT_MANAGER_STR].into_iter(),
                selector,
            ),
            ExtendedMoniker::ComponentInstance(moniker) => {
                moniker.matches_component_selector(selector)
            }
        }
    }

    fn sanitized(&self) -> String {
        match self {
            ExtendedMoniker::ComponentManager => EXTENDED_MONIKER_COMPONENT_MANAGER_STR.to_string(),
            ExtendedMoniker::ComponentInstance(moniker) => moniker.sanitized(),
        }
    }

    fn into_component_selector(self) -> ComponentSelector {
        ComponentSelector {
            moniker_segments: Some(
                match self {
                    ExtendedMoniker::ComponentManager => {
                        vec![EXTENDED_MONIKER_COMPONENT_MANAGER_STR.into()]
                    }
                    ExtendedMoniker::ComponentInstance(moniker) => {
                        moniker.path().iter().map(|value| value.to_string()).collect()
                    }
                }
                .into_iter()
                .map(StringSelector::ExactMatch)
                .collect(),
            ),
            ..Default::default()
        }
    }
}

impl SelectorExt for Moniker {
    fn match_against_selectors<'a>(
        &self,
        selectors: impl IntoIterator<Item = &'a Selector>,
    ) -> impl Iterator<Item = Result<&'a Selector, anyhow::Error>> {
        let s = SegmentIterator::from(self).collect::<Vec<_>>();
        match_component_moniker_against_selectors(s, selectors)
    }

    fn match_against_selectors_and_tree_name<'a>(
        &self,
        tree_name: &str,
        selectors: impl IntoIterator<Item = &'a Selector>,
    ) -> impl Iterator<Item = &'a Selector> {
        let m = SegmentIterator::from(self).collect::<Vec<_>>();

        selectors
            .into_iter()
            .filter(move |s| match_component_and_tree_name(&m, tree_name, s).unwrap_or(false))
    }

    fn match_against_component_selectors<'a, S>(
        &self,
        selectors: &'a [S],
    ) -> Result<Vec<&'a ComponentSelector>, anyhow::Error>
    where
        S: Borrow<ComponentSelector>,
    {
        let s = SegmentIterator::from(self).collect::<Vec<_>>();
        match_moniker_against_component_selectors(&s, selectors)
    }

    fn matches_selector(&self, selector: &Selector) -> Result<bool, anyhow::Error> {
        let s = SegmentIterator::from(self).collect::<Vec<_>>();
        match_component_moniker_against_selector(&s, selector)
    }

    fn matches_component_selector(
        &self,
        selector: &ComponentSelector,
    ) -> Result<bool, anyhow::Error> {
        match_moniker_against_component_selector(SegmentIterator::from(self), selector)
    }

    fn sanitized(&self) -> String {
        SegmentIterator::from(self)
            .map(|s| sanitize_string_for_selectors(&s).into_owned())
            .collect::<Vec<String>>()
            .join("/")
    }

    fn into_component_selector(self) -> ComponentSelector {
        ComponentSelector {
            moniker_segments: Some(
                self.path()
                    .iter()
                    .map(|value| StringSelector::ExactMatch(value.to_string()))
                    .collect(),
            ),
            ..Default::default()
        }
    }
}

enum SegmentIterator<'a> {
    Iter { path: Arc<[&'a BorrowedChildName]>, current_index: usize },
    Root(bool),
}

impl<'a> From<&'a Moniker> for SegmentIterator<'a> {
    fn from(moniker: &'a Moniker) -> Self {
        if moniker.is_root() {
            return SegmentIterator::Root(false);
        }
        SegmentIterator::Iter { path: moniker.path().into(), current_index: 0 }
    }
}

impl Iterator for SegmentIterator<'_> {
    type Item = String;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Iter { path, current_index } => {
                let segment = path.get(*current_index)?;
                let result = segment.to_string();
                *self = Self::Iter { path: path.clone(), current_index: *current_index + 1 };
                Some(result)
            }
            Self::Root(true) => None,
            Self::Root(done) => {
                *done = true;
                Some(ROOT_SEGMENT.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::prelude::*;
    use std::path::PathBuf;
    use std::str::FromStr;
    use tempfile::TempDir;
    use test_case::test_case;

    /// Loads all the selectors in the given directory.
    pub fn parse_selectors<E>(directory: &Path) -> Result<Vec<Selector>, Error>
    where
        E: for<'a> ParsingError<'a>,
    {
        let path: PathBuf = directory.to_path_buf();
        let mut selector_vec: Vec<Selector> = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.path().is_dir() {
                return Err(Error::NonFlatDirectory);
            } else {
                selector_vec.append(&mut parse_selector_file::<E>(&entry.path())?);
            }
        }
        Ok(selector_vec)
    }

    #[fuchsia::test]
    fn successful_selector_parsing() {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        File::create(tempdir.path().join("a.txt"))
            .expect("create file")
            .write_all(
                b"a:b:c

",
            )
            .expect("writing test file");
        File::create(tempdir.path().join("b.txt"))
            .expect("create file")
            .write_all(b"a*/b:c/d/*:*")
            .expect("writing test file");

        File::create(tempdir.path().join("c.txt"))
            .expect("create file")
            .write_all(
                b"// this is a comment
a:b:c
",
            )
            .expect("writing test file");
        File::create(tempdir.path().join("d.txt"))
            .expect("create file")
            .write_all(
                b"foo:bar
// This is a multi line comment.
//
// The blank comment line above is allowed.
foo:baz
",
            )
            .expect("writing test file");

        assert_matches::assert_matches!(parse_selectors::<VerboseError>(tempdir.path()), Ok(_));
    }

    #[fuchsia::test]
    fn unsuccessful_selector_parsing_bad_selector() {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        File::create(tempdir.path().join("a.txt"))
            .expect("create file")
            .write_all(b"a:b:c")
            .expect("writing test file");
        File::create(tempdir.path().join("b.txt"))
            .expect("create file")
            .write_all(b"**:**:**")
            .expect("writing test file");

        assert!(parse_selectors::<VerboseError>(tempdir.path()).is_err());
    }

    #[fuchsia::test]
    fn unsuccessful_selector_parsing_nonflat_dir() {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        File::create(tempdir.path().join("a.txt"))
            .expect("create file")
            .write_all(b"a:b:c")
            .expect("writing test file");
        File::create(tempdir.path().join("b.txt"))
            .expect("create file")
            .write_all(b"**:**:**")
            .expect("writing test file");

        std::fs::create_dir_all(tempdir.path().join("nested")).expect("make nested");
        File::create(tempdir.path().join("nested/c.txt"))
            .expect("create file")
            .write_all(b"**:**:**")
            .expect("writing test file");
        assert!(parse_selectors::<VerboseError>(tempdir.path()).is_err());
    }

    #[fuchsia::test]
    fn component_selector_match_test() {
        // Note: We provide the full selector syntax but this test is only validating it
        // against the provided moniker
        let passing_test_cases = vec![
            (r#"echo:*:*"#, vec!["echo"]),
            (r#"*/echo:*:*"#, vec!["abc", "echo"]),
            (r#"ab*/echo:*:*"#, vec!["abc", "echo"]),
            (r#"ab*/echo:*:*"#, vec!["abcde", "echo"]),
            (r#"*/ab*/echo:*:*"#, vec!["123", "abcde", "echo"]),
            (r#"echo*:*:*"#, vec!["echo"]),
            (r#"a/echo*:*:*"#, vec!["a", "echo1"]),
            (r#"a/echo*:*:*"#, vec!["a", "echo"]),
            (r#"ab*/echo:*:*"#, vec!["ab", "echo"]),
            (r#"a/**:*:*"#, vec!["a", "echo"]),
            (r#"a/**:*:*"#, vec!["a", "b", "echo"]),
        ];

        for (selector, moniker) in passing_test_cases {
            let parsed_selector = parse_selector::<VerboseError>(selector).unwrap();
            assert!(
                match_component_moniker_against_selector(&moniker, &parsed_selector).unwrap(),
                "Selector {selector:?} failed to match {moniker:?}"
            );
        }

        // Note: We provide the full selector syntax but this test is only validating it
        // against the provided moniker
        let failing_test_cases = vec![
            (r#"*:*:*"#, vec!["a", "echo"]),
            (r#"*/echo:*:*"#, vec!["123", "abc", "echo"]),
            (r#"a/**:*:*"#, vec!["b", "echo"]),
            (r#"e/**:*:*"#, vec!["echo"]),
        ];

        for (selector, moniker) in failing_test_cases {
            let parsed_selector = parse_selector::<VerboseError>(selector).unwrap();
            assert!(
                !match_component_moniker_against_selector(&moniker, &parsed_selector).unwrap(),
                "Selector {selector:?} matched {moniker:?}, but was expected to fail"
            );
        }
    }

    #[fuchsia::test]
    fn multiple_component_selectors_match_test() {
        let selectors = vec![r#"*/echo"#, r#"ab*/echo"#, r#"abc/m*"#];
        let moniker = vec!["abc".to_string(), "echo".to_string()];

        let component_selectors = selectors
            .into_iter()
            .map(|selector| parse_component_selector::<VerboseError>(selector).unwrap())
            .collect::<Vec<_>>();

        let match_res =
            match_moniker_against_component_selectors(moniker.as_slice(), &component_selectors[..]);
        assert!(match_res.is_ok());
        assert_eq!(match_res.unwrap().len(), 2);
    }

    #[test_case("a/b:c:d", "a/b:c:d" ; "no_wrap_with_basic_full_selector")]
    #[test_case("a/b:c", "a/b:c" ; "no_wrap_with_basic_partial_selector")]
    #[test_case(r"a/b:c/d\/e:f", r#"a/b:c/d\/e:f"# ; "no_wrap_with_escaped_forward_slash")]
    #[test_case(r"a/b:[name=root]c:d", "a/b:c:d" ; "no_wrap_with_default_name")]
    #[test_case(r"a/b:[name=cd-e]f:g", r#"a/b:[name=cd-e]f:g"# ; "no_wrap_with_non_default_name")]
    #[test_case(
        r#"a:[name="bc-d"]e:f"#,
        r"a:[name=bc-d]e:f"
        ; "no_wrap_with_unneeded_name_quotes"
    )]
    #[test_case(
        r#"a:[name="b[]c"]d:e"#,
        r#"a:[name="b[]c"]d:e"#
        ; "no_wrap_with_needed_name_quotes"
    )]
    #[test_case("a/b:[...]c:d", r#"a/b:[...]c:d"# ; "no_wrap_with_all_names")]
    #[test_case(
        r#"a/b:[name=c, name="d", name="f[]g"]h:i"#,
        r#"a/b:[name=c,name=d,name="f[]g"]h:i"#
        ; "no_wrap_with_name_list"
    )]
    #[test_case(r"a\:b/c:d:e", r"a\:b/c:d:e" ; "no_wrap_with_collection")]
    #[test_case(r"a/b/c*d:e:f", r#"a/b/c*d:e:f"# ; "no_wrap_with_wildcard_component")]
    #[test_case(r"a/b:c*/d:e", r#"a/b:c*/d:e"# ; "no_wrap_with_wildcard_tree")]
    #[test_case(r"a/b:c\*/d:e", r#"a/b:c\*/d:e"# ; "no_wrap_with_escaped_wildcard_tree")]
    #[test_case(r"a/b/c/d:e/f:g*", r#"a/b/c/d:e/f:g*"# ; "no_wrap_with_wildcard_property")]
    #[test_case(r"a/b/c/d:e/f:g*", r#"a/b/c/d:e/f:g*"# ; "no_wrap_with_escaped_wildcard_property")]
    #[test_case("a/b/c/d:e/f/g/h:k", "a/b/c/d:e/f/g/h:k" ; "no_wrap_with_deep_nesting")]
    #[fuchsia::test]
    fn selector_to_string_test_never_wrap(input: &str, expected: &str) {
        let selector = parse_verbose(input).unwrap();
        assert_eq!(
            selector_to_string(&selector, SelectorDisplayOptions::never_wrap_in_quotes()).unwrap(),
            expected,
            "left: actual, right: expected"
        );
    }

    #[test_case("a/b:c:d", "a/b:c:d" ; "with_basic_full_selector")]
    #[test_case("a/b:c", "a/b:c" ; "with_basic_partial_selector")]
    #[test_case(r"a/b:c/d\/e:f", r#""a/b:c/d\/e:f""# ; "with_escaped_forward_slash")]
    #[test_case(r"a/b:[name=root]c:d", "a/b:c:d" ; "with_default_name")]
    #[test_case(r"a/b:[name=cd-e]f:g", r#""a/b:[name=cd-e]f:g""# ; "with_non_default_name")]
    #[test_case(r#"a:[name="bc-d"]e:f"#, r#""a:[name=bc-d]e:f""# ; "with_unneeded_name_quotes")]
    #[test_case(r#"a:[name="b[]c"]d:e"#, r#""a:[name="b[]c"]d:e""# ; "with_needed_name_quotes")]
    #[test_case("a/b:[...]c:d", r#""a/b:[...]c:d""# ; "with_all_names")]
    #[test_case(
        r#"a/b:[name=c, name="d", name="f[]g"]h:i"#,
        r#""a/b:[name=c,name=d,name="f[]g"]h:i""#
        ; "with_name_list"
    )]
    #[test_case(r"a\:b/c:d:e", r"a\:b/c:d:e" ; "with_collection")]
    #[test_case(r"a/b/c*d:e:f", r#""a/b/c*d:e:f""# ; "with_wildcard_component")]
    #[test_case(r"a/b:c*/d:e", r#""a/b:c*/d:e""# ; "with_wildcard_tree")]
    #[test_case(r"a/b:c\*/d:e", r#""a/b:c\*/d:e""# ; "with_escaped_wildcard_tree")]
    #[test_case(r"a/b/c/d:e/f:g*", r#""a/b/c/d:e/f:g*""# ; "with_wildcard_property")]
    #[test_case(r"a/b/c/d:e/f:g*", r#""a/b/c/d:e/f:g*""# ; "with_escaped_wildcard_property")]
    #[test_case("a/b/c/d:e/f/g/h:k", "a/b/c/d:e/f/g/h:k" ; "with_deep_nesting")]
    #[fuchsia::test]
    fn selector_to_string_test_default(input: &str, expected: &str) {
        let selector = parse_verbose(input).unwrap();
        assert_eq!(
            selector_to_string(&selector, SelectorDisplayOptions::default()).unwrap(),
            expected,
            "left: actual, right: expected"
        );
    }

    #[test_case("a*", r"a\*" ; "when_star_not_leading")]
    #[test_case("a:", r"a\:" ; "when_colon_not_leading")]
    #[test_case(":", r"\:" ; "when_colon_leading")]
    #[test_case("*", r"\*" ; "when_star_leading")]
    #[test_case(r"*:\abc", r"\*\:\\abc" ; "when_mixed_with_leading_special_chars")]
    #[fuchsia::test]
    fn sanitize_string_for_selectors_works(input: &str, expected: &str) {
        assert_eq!(sanitize_string_for_selectors(input), expected);
    }

    #[fuchsia::test]
    fn sanitize_moniker_for_selectors_result_is_usable() {
        let selector = parse_selector::<VerboseError>(&format!(
            "{}:root",
            sanitize_moniker_for_selectors("foo/coll:bar/baz")
        ))
        .unwrap();
        let component_selector = selector.component_selector.as_ref().unwrap();
        let moniker = ["foo", "coll:bar", "baz"];
        assert!(
            match_moniker_against_component_selector(moniker.iter(), component_selector).unwrap()
        );
    }

    #[fuchsia::test]
    fn escaped_spaces() {
        let selector_str = "foo:bar\\ baz/a*\\ b:quux";
        let selector = parse_selector::<VerboseError>(selector_str).unwrap();
        assert_eq!(
            selector,
            Selector {
                component_selector: Some(ComponentSelector {
                    moniker_segments: Some(vec![StringSelector::ExactMatch("foo".into()),]),
                    ..Default::default()
                }),
                tree_selector: Some(TreeSelector::PropertySelector(PropertySelector {
                    node_path: vec![
                        StringSelector::ExactMatch("bar baz".into()),
                        StringSelector::StringPattern("a* b".into()),
                    ],
                    target_properties: StringSelector::ExactMatch("quux".into())
                })),
                ..Default::default()
            }
        );
    }

    #[fuchsia::test]
    fn match_string_test() {
        // Exact match.
        assert!(match_string(&StringSelector::ExactMatch("foo".into()), "foo"));

        // Valid pattern matches.
        assert!(match_string(&StringSelector::StringPattern("*foo*".into()), "hellofoobye"));
        assert!(match_string(&StringSelector::StringPattern("bar*foo".into()), "barxfoo"));
        assert!(match_string(&StringSelector::StringPattern("bar*foo".into()), "barfoo"));
        assert!(match_string(&StringSelector::StringPattern("bar*foo".into()), "barxfoo"));
        assert!(match_string(&StringSelector::StringPattern("foo*".into()), "foobar"));
        assert!(match_string(&StringSelector::StringPattern("*".into()), "foo"));
        assert!(match_string(&StringSelector::StringPattern("bar*baz*foo".into()), "barxzybazfoo"));
        assert!(match_string(&StringSelector::StringPattern("foo*bar*baz".into()), "foobazbarbaz"));

        // Escaped char.
        assert!(match_string(&StringSelector::StringPattern("foo\\*".into()), "foo*"));

        // Invalid cases.
        assert!(!match_string(&StringSelector::StringPattern("foo\\".into()), "foo\\"));
        assert!(!match_string(&StringSelector::StringPattern("bar*foo".into()), "barxfoox"));
        assert!(!match_string(&StringSelector::StringPattern("m*".into()), "echo.csx"));
        assert!(!match_string(&StringSelector::StringPattern("*foo*".into()), "xbary"));
        assert!(!match_string(
            &StringSelector::StringPattern("foo*bar*baz*qux".into()),
            "foobarbaazqux"
        ));
    }

    #[fuchsia::test]
    fn test_log_interest_selector() {
        assert_eq!(
            parse_log_interest_selector("core/network#FATAL").unwrap(),
            LogInterestSelector {
                selector: parse_component_selector::<VerboseError>("core/network").unwrap(),
                interest: Interest { min_severity: Some(Severity::Fatal), ..Default::default() }
            }
        );
        assert_eq!(
            parse_log_interest_selector("any/component#INFO").unwrap(),
            LogInterestSelector {
                selector: parse_component_selector::<VerboseError>("any/component").unwrap(),
                interest: Interest { min_severity: Some(Severity::Info), ..Default::default() }
            }
        );
        assert_eq!(
            parse_log_interest_selector("any/coll:instance/foo#INFO").unwrap(),
            LogInterestSelector {
                selector: parse_component_selector::<VerboseError>("any/coll\\:instance/foo")
                    .unwrap(),
                interest: Interest { min_severity: Some(Severity::Info), ..Default::default() }
            }
        );
        assert_eq!(
            parse_log_interest_selector("any/coll:*/foo#INFO").unwrap(),
            LogInterestSelector {
                selector: parse_component_selector::<VerboseError>("any/coll\\:*/foo").unwrap(),
                interest: Interest { min_severity: Some(Severity::Info), ..Default::default() }
            }
        );
    }
    #[test]
    fn test_log_interest_selector_error() {
        assert!(parse_log_interest_selector("anything////#FATAL").is_err());
        assert!(parse_log_interest_selector("core/network").is_err());
        assert!(parse_log_interest_selector("core/network#FAKE").is_err());
        assert!(parse_log_interest_selector("core/network\\:foo#FAKE").is_err());
    }

    #[test]
    fn test_moniker_to_selector() {
        assert_eq!(
            Moniker::from_str("a/b/c").unwrap().into_component_selector(),
            parse_component_selector::<VerboseError>("a/b/c").unwrap()
        );
        assert_eq!(
            ExtendedMoniker::ComponentManager.into_component_selector(),
            parse_component_selector::<VerboseError>("<component_manager>").unwrap()
        );
        assert_eq!(
            ExtendedMoniker::ComponentInstance(Moniker::from_str("a/b/c").unwrap())
                .into_component_selector(),
            parse_component_selector::<VerboseError>("a/b/c").unwrap()
        );
        assert_eq!(
            ExtendedMoniker::ComponentInstance(Moniker::from_str("a/coll:id/c").unwrap())
                .into_component_selector(),
            parse_component_selector::<VerboseError>("a/coll\\:id/c").unwrap()
        );
    }

    #[test]
    fn test_parse_log_interest_or_severity() {
        for (severity_str, severity) in [
            ("TRACE", Severity::Trace),
            ("DEBUG", Severity::Debug),
            ("INFO", Severity::Info),
            ("WARN", Severity::Warn),
            ("ERROR", Severity::Error),
            ("FATAL", Severity::Fatal),
        ] {
            assert_eq!(
                parse_log_interest_selector_or_severity(severity_str).unwrap(),
                LogInterestSelector {
                    selector: parse_component_selector::<VerboseError>("**").unwrap(),
                    interest: Interest { min_severity: Some(severity), ..Default::default() }
                }
            );
        }

        assert_eq!(
            parse_log_interest_selector_or_severity("foo/bar#DEBUG").unwrap(),
            LogInterestSelector {
                selector: parse_component_selector::<VerboseError>("foo/bar").unwrap(),
                interest: Interest { min_severity: Some(Severity::Debug), ..Default::default() }
            }
        );

        assert!(parse_log_interest_selector_or_severity("RANDOM").is_err());
        assert!(parse_log_interest_selector_or_severity("core/foo#NO#YES").is_err());
    }

    #[test]
    fn test_parse_tree_selector() {
        let selector = parse_tree_selector::<VerboseError>("root/node*/nested:prop").unwrap();
        assert_eq!(
            selector,
            TreeSelector::PropertySelector(PropertySelector {
                node_path: vec![
                    StringSelector::ExactMatch("root".into()),
                    StringSelector::StringPattern("node*".into()),
                    StringSelector::ExactMatch("nested".into()),
                ],
                target_properties: StringSelector::ExactMatch("prop".into())
            }),
        );
    }

    #[test]
    fn test_monikers_against_selectors_and_tree_name() {
        let selectors = &[
            parse_selector::<VerboseError>("core/foo:root:prop").unwrap(),
            parse_selector::<VerboseError>("core/*:[name=root]root:prop").unwrap(),
            parse_selector::<VerboseError>("core/baz:[name=baz]root:prop").unwrap(),
            parse_selector::<VerboseError>("core/baz:[name=root]root:prop").unwrap(),
            parse_selector::<VerboseError>("core/*:[...]root:prop").unwrap(),
            parse_selector::<VerboseError>("<component_manager>:root:prop").unwrap(),
        ];

        {
            let foo = ExtendedMoniker::try_from("core/foo").unwrap();

            let actual = foo
                .match_against_selectors_and_tree_name("root", selectors.iter())
                .collect::<Vec<_>>();
            assert_eq!(actual, vec![&selectors[0], &selectors[1], &selectors[4]]);

            let foo = Moniker::try_from("core/foo").unwrap();

            let actual = foo
                .match_against_selectors_and_tree_name("root", selectors.iter())
                .collect::<Vec<_>>();
            assert_eq!(actual, vec![&selectors[0], &selectors[1], &selectors[4]]);
        }

        {
            let baz = ExtendedMoniker::try_from("core/baz").unwrap();

            let actual = baz
                .match_against_selectors_and_tree_name("root", selectors.iter())
                .collect::<Vec<_>>();
            assert_eq!(actual, vec![&selectors[1], &selectors[3], &selectors[4]]);

            let baz = Moniker::try_from("core/baz").unwrap();

            let actual = baz
                .match_against_selectors_and_tree_name("root", selectors.iter())
                .collect::<Vec<_>>();
            assert_eq!(actual, vec![&selectors[1], &selectors[3], &selectors[4]]);
        }

        {
            let baz = ExtendedMoniker::try_from("core/baz").unwrap();

            let actual = baz
                .match_against_selectors_and_tree_name("baz", selectors.iter())
                .collect::<Vec<_>>();
            assert_eq!(actual, vec![&selectors[2], &selectors[4]]);

            let baz = Moniker::try_from("core/baz").unwrap();

            let actual = baz
                .match_against_selectors_and_tree_name("baz", selectors.iter())
                .collect::<Vec<_>>();
            assert_eq!(actual, vec![&selectors[2], &selectors[4]]);
        }

        {
            let qux = ExtendedMoniker::try_from("core/qux").unwrap();

            let actual = qux
                .match_against_selectors_and_tree_name("qux", selectors.iter())
                .collect::<Vec<_>>();
            assert_eq!(actual, vec![&selectors[4]]);

            let qux = Moniker::try_from("core/qux").unwrap();

            let actual = qux
                .match_against_selectors_and_tree_name("qux", selectors.iter())
                .collect::<Vec<_>>();
            assert_eq!(actual, vec![&selectors[4]]);
        }

        {
            let cm = ExtendedMoniker::try_from(EXTENDED_MONIKER_COMPONENT_MANAGER_STR).unwrap();

            let actual = cm
                .match_against_selectors_and_tree_name("root", selectors.iter())
                .collect::<Vec<_>>();
            assert_eq!(actual, vec![&selectors[5]]);
        }

        {
            let sanitized = sanitize_string_for_selectors("🦀:test");
            assert_eq!(sanitized, "🦀\\:test");

            let sanitized_no_special = sanitize_string_for_selectors("🦀🦀🦀");
            assert_eq!(sanitized_no_special, "🦀🦀🦀");

            assert!(match_pattern("🦀*", "🦀test"));
            assert!(!match_pattern("🦀*", "test"));
        }
    }
}
