// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::ParseError;
use crate::ir::*;
use crate::validate::{ValidateComponentSelectorExt, ValidateExt, ValidateTreeSelectorExt};
use bitflags::bitflags;

use winnow::Parser;
use winnow::ascii::{multispace0, take_escaped};
use winnow::combinator::{alt, cond, eof, opt, preceded, separated};
use winnow::error::{ErrMode, ParserError};
use winnow::token::{none_of, one_of, take_while};

const ALL_TREE_NAMES_SELECTED_SYMBOL: &str = "...";

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct RequireEscaped: u8 {
        const NONE = 0;
        const COLONS = 1;
        const WHITESPACE = 2;
    }
}

/// Recognizes 0 or more spaces or tabs.
fn whitespace0<'a, E>(input: &mut &'a str) -> Result<&'a str, ErrMode<E>>
where
    E: ParserError<&'a str>,
{
    take_while(0.., (' ', '\t')).parse_next(input)
}

/// Parses an input containing any number and type of whitespace at the front.
fn spaced<'a, E, F, O>(parser: F) -> impl Parser<&'a str, O, ErrMode<E>>
where
    F: Parser<&'a str, O, ErrMode<E>>,
    E: ParserError<&'a str>,
{
    preceded(whitespace0::<E>, parser)
}

fn tree_name_item<'a, E>(input: &mut &'a str) -> Result<&'a str, ErrMode<E>>
where
    E: ParserError<&'a str>,
{
    let value_parser = alt((
        winnow::combinator::delimited(
            '"',
            take_escaped(none_of(['\\', '"']), '\\', one_of(['"', '*', '/', ':', ' '])),
            '"',
        ),
        take_while(1.., |c| c != ',' && c != ']'),
    ));
    let (_, _, value) = spaced(("name", "=", value_parser)).parse_next(input)?;
    Ok(value)
}

fn conjoined_tree_names<'a, E>() -> impl Parser<&'a str, Option<TreeNames<'a>>, ErrMode<E>>
where
    E: winnow::error::ParserError<&'a str>,
{
    opt(winnow::combinator::delimited(
        '[',
        alt((
            spaced(ALL_TREE_NAMES_SELECTED_SYMBOL).map(|_| TreeNames::All),
            separated(1.., tree_name_item::<E>, spaced(",")).map(|items: Vec<&str>| items.into()),
        )),
        ']',
    ))
}

fn extract_from_quotes(input: &str) -> &str {
    if input.starts_with('"') && input.ends_with('"') && input.len() >= 2 {
        &input[1..input.len() - 1]
    } else {
        input
    }
}

/// Returns the parser for a tree selector, which is a node selector and an optional property selector.
fn tree_selector<'a, E>(
    required_escapes: RequireEscaped,
) -> impl Parser<&'a str, TreeSelector<'a>, ErrMode<E>>
where
    E: ParserError<&'a str>,
{
    move |input: &mut &'a str| {
        let mut esc = move |input: &mut &'a str| {
            if required_escapes.intersects(RequireEscaped::WHITESPACE) {
                take_escaped(
                    none_of([':', '/', '\\', ' ', '\t', '\n']),
                    '\\',
                    one_of(['*', ' ', '\t', '/', ':', '\\']),
                )
                .parse_next(input)
            } else {
                take_escaped(
                    none_of([':', '/', '\\', '\t', '\n']),
                    '\\',
                    one_of(['*', ' ', '\t', '/', ':', '\\']),
                )
                .parse_next(input)
            }
        };

        let tree_names = conjoined_tree_names::<E>().parse_next(input)?;

        let node_segments: Vec<&str> = separated(1.., esc.by_ref(), "/").parse_next(input)?;
        let property_segment: Option<&str> = opt(winnow::combinator::preceded(
            ":",
            esc.by_ref().verify(|value: &str| !value.is_empty()),
        ))
        .parse_next(input)?;
        Ok(TreeSelector {
            node: node_segments.into_iter().map(|value| value.into()).collect(),
            property: property_segment.map(|value| value.into()),
            tree_names,
        })
    }
}

/// Returns the parser for a component selector. The parser accepts unescaped depending on the
/// the argument `escape_colons`.
fn component_selector<'a, E>(
    required_escapes: RequireEscaped,
) -> impl Parser<&'a str, ComponentSelector<'a>, ErrMode<E>>
where
    E: ParserError<&'a str>,
{
    move |input: &mut &'a str| {
        let segments: Vec<&str> = if required_escapes.intersects(RequireEscaped::COLONS) {
            let mut segment = take_escaped(
                take_while(1.., ('a'..='z', 'A'..='Z', '0'..='9', '*', '.', '-', '_', '>', '<')),
                '\\',
                ":",
            );
            winnow::combinator::preceded(
                opt(alt(("./", "/"))),
                separated(1.., segment.by_ref(), "/"),
            )
            .parse_next(input)?
        } else {
            let mut segment = take_while(
                1..,
                ('a'..='z', 'A'..='Z', '0'..='9', '*', '.', '-', '_', '>', '<', ':'),
            );
            winnow::combinator::preceded(
                opt(alt(("./", "/"))),
                separated(1.., segment.by_ref(), "/"),
            )
            .parse_next(input)?
        };
        Ok(ComponentSelector { segments: segments.into_iter().map(Segment::from).collect() })
    }
}

fn comment<'a, E>(input: &mut &'a str) -> Result<&'a str, ErrMode<E>>
where
    E: ParserError<&'a str>,
{
    let comment = spaced(winnow::combinator::preceded(
        "//",
        take_while(0.., |c: char| c != '\n' && c != '\r'),
    ))
    .parse_next(input)?;
    if !input.is_empty() {
        let _ = one_of(['\n', '\r']).parse_next(input)?;
    }
    Ok(comment)
}

/// Parses a core selector (component + tree + property). It accepts both raw selectors or
/// selectors wrapped in double quotes. Selectors wrapped in quotes accept spaces in the tree and
/// property names and require internal quotes to be escaped.
fn core_selector<'a, E>(
    input: &mut &'a str,
) -> Result<(ComponentSelector<'a>, TreeSelector<'a>), ErrMode<E>>
where
    E: ParserError<&'a str>,
{
    let input_str = *input;
    let required_tree_escape = if input_str.starts_with('"') {
        RequireEscaped::empty()
    } else {
        RequireEscaped::WHITESPACE
    };
    let unwrapped = extract_from_quotes(input_str);
    let mut unwrapped_input = unwrapped;
    let (component, _, tree, _, _) = (
        component_selector::<E>(RequireEscaped::COLONS),
        ":",
        tree_selector::<E>(required_tree_escape),
        whitespace0::<E>,
        eof,
    )
        .parse_next(&mut unwrapped_input)?;
    *input = "";
    Ok((component, tree))
}

/// Recognizes selectors, with comments allowed or disallowed.
fn do_parse_selector<'a, E>(
    allow_inline_comment: bool,
) -> impl Parser<&'a str, Selector<'a>, ErrMode<E>>
where
    E: ParserError<&'a str>,
{
    (spaced(core_selector::<E>), cond(allow_inline_comment, opt(comment::<E>)), whitespace0::<E>)
        .map(|((component, tree), _, _)| Selector { component, tree })
}

/// A fast efficient error that won't provide much information besides the name kind of nom parsers
/// that failed and the position at which it failed.
pub struct FastError;

/// A slower but more user friendly error that will provide information about the chain of parsers
/// that found the error and some context.
pub struct VerboseError;

mod private {
    pub trait Sealed {}

    impl Sealed for super::FastError {}
    impl Sealed for super::VerboseError {}
}

/// Implemented by types which can be used to specify the error strategy the parsers should use.
pub trait ParsingError<'a>: private::Sealed {
    type Internal: ParserError<&'a str>;

    fn to_error(input: &str, err: ErrMode<Self::Internal>) -> ParseError;
}

impl<'a> ParsingError<'a> for FastError {
    type Internal = winnow::error::InputError<&'a str>;

    fn to_error(_: &str, err: ErrMode<Self::Internal>) -> ParseError {
        let e = err.into_inner().unwrap();
        ParseError::Fast { input: e.input.to_string() }
    }
}

impl<'a> ParsingError<'a> for VerboseError {
    type Internal = winnow::error::ContextError;

    fn to_error(_input: &str, err: ErrMode<Self::Internal>) -> ParseError {
        ParseError::Verbose(format!("{:?}", err.into_inner().unwrap()))
    }
}

/// Parses the input into a `Selector`.
pub fn selector<'a, E>(input: &'a str) -> Result<Selector<'a>, ParseError>
where
    E: ParsingError<'a>,
{
    let mut input_ref = input;
    let result = (do_parse_selector::<E::Internal>(false), eof).parse_next(&mut input_ref);
    match result {
        Ok((selector, _)) => {
            selector.validate()?;
            Ok(selector)
        }
        Err(e) => Err(E::to_error(input, e)),
    }
}

/// Parses the input into a `TreeSelector` ignoring any whitespace around the component
/// selector.
pub fn standalone_tree_selector<'a, E>(input: &'a str) -> Result<TreeSelector<'a>, ParseError>
where
    E: ParsingError<'a>,
{
    let required_tree_escape =
        if input.starts_with('"') { RequireEscaped::empty() } else { RequireEscaped::WHITESPACE };
    let unwrapped = extract_from_quotes(input);

    let mut input_ref = unwrapped;
    let result = (spaced(tree_selector::<E::Internal>(required_tree_escape)), multispace0, eof)
        .parse_next(&mut input_ref);
    match result {
        Ok((tree_selector, _, _)) => {
            tree_selector.validate()?;
            Ok(tree_selector)
        }
        Err(e) => Err(E::to_error(input, e)),
    }
}

/// Parses the input into a `ComponentSelector` ignoring any whitespace around the component
/// selector.
pub fn consuming_component_selector<'a, E>(
    input: &'a str,
    required_escapes: RequireEscaped,
) -> Result<ComponentSelector<'a>, ParseError>
where
    E: ParsingError<'a>,
{
    let mut input_ref = input;
    let result = (spaced(component_selector::<E::Internal>(required_escapes)), multispace0, eof)
        .parse_next(&mut input_ref);
    match result {
        Ok((component_selector, _, _)) => {
            component_selector.validate()?;
            Ok(component_selector)
        }
        Err(e) => Err(E::to_error(input, e)),
    }
}

/// Parses the given input line into a Selector or None.
pub fn selector_or_comment<'a, E>(input: &'a str) -> Result<Option<Selector<'a>>, ParseError>
where
    E: ParsingError<'a>,
{
    let mut input_ref = input;
    let maybe_selector: Option<Selector<'a>> = match comment::<E::Internal>(&mut input_ref) {
        Ok(_) => Ok(None),
        Err(ErrMode::Backtrack(_)) => {
            do_parse_selector::<E::Internal>(true).parse_next(&mut input_ref).map(Some)
        }
        Err(e) => Err(e),
    }
    .map_err(|e| E::to_error(input, e))?;

    let _: &str = eof.parse_next(&mut input_ref).map_err(|e| E::to_error(input, e))?;

    if let Some(selector) = maybe_selector {
        selector.validate()?;
        Ok(Some(selector))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn canonical_component_selector_test() {
        let test_vector = vec![
            (
                "a/b/c",
                vec![
                    Segment::ExactMatch("a".into()),
                    Segment::ExactMatch("b".into()),
                    Segment::ExactMatch("c".into()),
                ],
            ),
            (
                "a/*/c",
                vec![
                    Segment::ExactMatch("a".into()),
                    Segment::Pattern("*".into()),
                    Segment::ExactMatch("c".into()),
                ],
            ),
            (
                "a/b*/c",
                vec![
                    Segment::ExactMatch("a".into()),
                    Segment::Pattern("b*".into()),
                    Segment::ExactMatch("c".into()),
                ],
            ),
            (
                "a/b/**",
                vec![
                    Segment::ExactMatch("a".into()),
                    Segment::ExactMatch("b".into()),
                    Segment::Pattern("**".into()),
                ],
            ),
            (
                "core/session\\:id/foo",
                vec![
                    Segment::ExactMatch("core".into()),
                    Segment::ExactMatch("session:id".into()),
                    Segment::ExactMatch("foo".into()),
                ],
            ),
            ("c", vec![Segment::ExactMatch("c".into())]),
            ("<component_manager>", vec![Segment::ExactMatch("<component_manager>".into())]),
            (
                r#"a/*/b/**"#,
                vec![
                    Segment::ExactMatch("a".into()),
                    Segment::Pattern("*".into()),
                    Segment::ExactMatch("b".into()),
                    Segment::Pattern("**".into()),
                ],
            ),
        ];

        for (test_string, expected_segments) in test_vector {
            let selector =
                component_selector::<winnow::error::ContextError>(RequireEscaped::COLONS)
                    .parse(test_string)
                    .unwrap();

            assert_eq!(expected_segments, selector.segments);

            // Component selectors can start with `/`
            let test_moniker_string = format!("/{test_string}");
            let selector =
                component_selector::<winnow::error::ContextError>(RequireEscaped::COLONS)
                    .parse(&test_moniker_string)
                    .unwrap();
            assert_eq!(expected_segments, selector.segments);

            // Component selectors can start with `./`
            let test_moniker_string = format!("./{test_string}");
            let selector =
                component_selector::<winnow::error::ContextError>(RequireEscaped::COLONS)
                    .parse(&test_moniker_string)
                    .unwrap();
            assert_eq!(expected_segments, selector.segments);

            // We can also accept component selectors without escaping
            let test_moniker_string = test_string.replace("\\:", ":");
            let selector =
                component_selector::<winnow::error::ContextError>(RequireEscaped::empty())
                    .parse(&test_moniker_string)
                    .unwrap();
            assert_eq!(expected_segments, selector.segments);
        }
    }

    #[fuchsia::test]
    fn missing_path_component_selector_test() {
        let component_selector_string = "c";
        let cs = component_selector::<winnow::error::ContextError>(RequireEscaped::COLONS)
            .parse(component_selector_string)
            .unwrap();

        let mut path_vec = cs.segments;
        assert_eq!(path_vec.pop(), Some(Segment::ExactMatch("c".into())));
        assert!(path_vec.is_empty());
    }

    #[fuchsia::test]
    fn errorful_component_selector_test() {
        let test_vector: Vec<&str> = vec![
            "",
            "a\\",
            r#"a/b***/c"#,
            r#"a/***/c"#,
            r#"a/**/c"#,
            // NOTE: This used to be accepted but not anymore. Spaces shouldn't be a valid component
            // selector character since it's not a valid moniker character.
            " ",
            // NOTE: The previous parser was accepting quotes in component selectors. However, by
            // definition, a component moniker (both in v1 and v2) doesn't allow a `*` in its name.
            r#"a/b\*/c"#,
            r#"a/\*/c"#,
            // Invalid characters
            "a$c/d",
        ];
        for test_string in test_vector {
            let component_selector_result =
                consuming_component_selector::<VerboseError>(test_string, RequireEscaped::COLONS);
            assert!(component_selector_result.is_err(), "expected '{test_string}' to fail");
        }
    }

    #[fuchsia::test]
    fn canonical_tree_selector_test() {
        let test_vector = vec![
            (
                r#"[name="with internal ,"]b/c:d"#,
                vec![Segment::ExactMatch("b".into()), Segment::ExactMatch("c".into())],
                Some(Segment::ExactMatch("d".into())),
                Some(vec![r#"with internal ,"#].into()),
            ),
            (
                r#"[name="with internal \" escaped quote"]b/c:d"#,
                vec![Segment::ExactMatch("b".into()), Segment::ExactMatch("c".into())],
                Some(Segment::ExactMatch("d".into())),
                Some(vec![r#"with internal " escaped quote"#].into()),
            ),
            (
                r#"[name="with internal ] closing bracket"]b/c:d"#,
                vec![Segment::ExactMatch("b".into()), Segment::ExactMatch("c".into())],
                Some(Segment::ExactMatch("d".into())),
                Some(vec!["with internal ] closing bracket"].into()),
            ),
            (
                "[name=a]b/c:d",
                vec![Segment::ExactMatch("b".into()), Segment::ExactMatch("c".into())],
                Some(Segment::ExactMatch("d".into())),
                Some(vec!["a"].into()),
            ),
            (
                "[name=a:b:c:d]b/c:d",
                vec![Segment::ExactMatch("b".into()), Segment::ExactMatch("c".into())],
                Some(Segment::ExactMatch("d".into())),
                Some(vec!["a:b:c:d"].into()),
            ),
            (
                "[name=a,name=bb]b/c:d",
                vec![Segment::ExactMatch("b".into()), Segment::ExactMatch("c".into())],
                Some(Segment::ExactMatch("d".into())),
                Some(vec!["a", "bb"].into()),
            ),
            (
                "[...]b/c:d",
                vec![Segment::ExactMatch("b".into()), Segment::ExactMatch("c".into())],
                Some(Segment::ExactMatch("d".into())),
                Some(TreeNames::All),
            ),
            (
                "[name=a, name=bb]b/c:d",
                vec![Segment::ExactMatch("b".into()), Segment::ExactMatch("c".into())],
                Some(Segment::ExactMatch("d".into())),
                Some(vec!["a", "bb"].into()),
            ),
            (
                "[name=a, name=\"bb\"]b/c:d",
                vec![Segment::ExactMatch("b".into()), Segment::ExactMatch("c".into())],
                Some(Segment::ExactMatch("d".into())),
                Some(vec!["a", "bb"].into()),
            ),
            (
                r#"[name=a, name="a/\*:a"]b/c:d"#,
                vec![Segment::ExactMatch("b".into()), Segment::ExactMatch("c".into())],
                Some(Segment::ExactMatch("d".into())),
                Some(vec!["a", "a/*:a"].into()),
            ),
            (
                r#""a 1/b:d""#,
                vec![Segment::ExactMatch("a 1".into()), Segment::ExactMatch("b".into())],
                Some(Segment::ExactMatch("d".into())),
                None,
            ),
            (
                r#""a 1/b 2:d""#,
                vec![Segment::ExactMatch("a 1".into()), Segment::ExactMatch("b 2".into())],
                Some(Segment::ExactMatch("d".into())),
                None,
            ),
            (
                r#""a 1/b 2:d 3""#,
                vec![Segment::ExactMatch("a 1".into()), Segment::ExactMatch("b 2".into())],
                Some(Segment::ExactMatch("d 3".into())),
                None,
            ),
            (
                r#"a\ 1/b:d"#,
                vec![Segment::ExactMatch("a 1".into()), Segment::ExactMatch("b".into())],
                Some(Segment::ExactMatch("d".into())),
                None,
            ),
            (
                r#"a\ 1/b\ 2:d"#,
                vec![Segment::ExactMatch("a 1".into()), Segment::ExactMatch("b 2".into())],
                Some(Segment::ExactMatch("d".into())),
                None,
            ),
            (
                r#"a\ 1/b\ 2:d\ 3"#,
                vec![Segment::ExactMatch("a 1".into()), Segment::ExactMatch("b 2".into())],
                Some(Segment::ExactMatch("d 3".into())),
                None,
            ),
            (
                r#""a\ 1/b\ 2:d\ 3""#,
                vec![Segment::ExactMatch("a 1".into()), Segment::ExactMatch("b 2".into())],
                Some(Segment::ExactMatch("d 3".into())),
                None,
            ),
            (
                "a/b:c",
                vec![Segment::ExactMatch("a".into()), Segment::ExactMatch("b".into())],
                Some(Segment::ExactMatch("c".into())),
                None,
            ),
            (
                "a/*:c",
                vec![Segment::ExactMatch("a".into()), Segment::Pattern("*".into())],
                Some(Segment::ExactMatch("c".into())),
                None,
            ),
            (
                "a/b:*",
                vec![Segment::ExactMatch("a".into()), Segment::ExactMatch("b".into())],
                Some(Segment::Pattern("*".into())),
                None,
            ),
            (
                "a/b",
                vec![Segment::ExactMatch("a".into()), Segment::ExactMatch("b".into())],
                None,
                None,
            ),
            (
                r#"a/b\:\*c"#,
                vec![Segment::ExactMatch("a".into()), Segment::ExactMatch("b:*c".into())],
                None,
                None,
            ),
        ];

        for (string, expected_path, expected_property, expected_tree_name) in test_vector {
            let tree_selector = standalone_tree_selector::<VerboseError>(string)
                .unwrap_or_else(|e| panic!("input: |{string}| error: {e}"));
            assert_eq!(
                tree_selector,
                TreeSelector {
                    node: expected_path,
                    property: expected_property,
                    tree_names: expected_tree_name,
                },
                "input: |{string}|",
            );
        }
    }

    #[fuchsia::test]
    fn errorful_tree_selector_test() {
        let test_vector = vec![
            // Not allowed due to empty property selector.
            "a/b:",
            // Not allowed due to glob property selector.
            "a/b:**",
            // String literals can't have globs.
            r#"a/b**:c"#,
            // Property selector string literals cant have globs.
            r#"a/b:c**"#,
            "a/b:**",
            // Node path cant have globs.
            "a/**:c",
            // Node path can't be empty
            ":c",
            // Spaces aren't accepted when parsing with allow_spaces=false.
            "a b:c",
            "a*b:\tc",
        ];
        for string in test_vector {
            // prepend a placeholder component selector so that we exercise the validation code.
            let test_selector = format!("a:{string}");
            assert!(
                selector::<VerboseError>(&test_selector).is_err(),
                "{test_selector} should fail"
            );
        }
    }

    #[fuchsia::test]
    fn tree_selector_with_spaces() {
        let with_spaces = vec![
            (
                r#"a\ b:c"#,
                vec![Segment::ExactMatch("a b".into())],
                Some(Segment::ExactMatch("c".into())),
            ),
            (
                r#"ab/\ d:c\ "#,
                vec![Segment::ExactMatch("ab".into()), Segment::ExactMatch(" d".into())],
                Some(Segment::ExactMatch("c ".into())),
            ),
            (
                "a\\\t*b:c",
                vec![Segment::Pattern("a\t*b".into())],
                Some(Segment::ExactMatch("c".into())),
            ),
            (
                r#"a\ "x":c"#,
                vec![Segment::ExactMatch(r#"a "x""#.into())],
                Some(Segment::ExactMatch("c".into())),
            ),
        ];
        for (string, node, property) in with_spaces {
            let ts = (tree_selector::<()>(RequireEscaped::WHITESPACE), eof)
                .map(|(r, _)| r)
                .parse(string)
                .unwrap();
            assert_eq!(ts, TreeSelector { node, property, tree_names: None });
        }

        // Un-escaped quotes aren't accepted when parsing with spaces.
        assert!(standalone_tree_selector::<VerboseError>(r#"a/b:"xc"/d"#).is_err());
    }

    #[fuchsia::test]
    fn parse_full_selector() {
        assert_eq!(
            selector::<VerboseError>("core/**:some-node/he*re:prop").unwrap(),
            Selector {
                component: ComponentSelector {
                    segments: vec![
                        Segment::ExactMatch("core".into()),
                        Segment::Pattern("**".into()),
                    ],
                },
                tree: TreeSelector {
                    node: vec![
                        Segment::ExactMatch("some-node".into()),
                        Segment::Pattern("he*re".into()),
                    ],
                    property: Some(Segment::ExactMatch("prop".into())),
                    tree_names: None,
                },
            }
        );

        // Ignores whitespace.
        assert_eq!(
            selector::<VerboseError>("   foo:bar  ").unwrap(),
            Selector {
                component: ComponentSelector { segments: vec![Segment::ExactMatch("foo".into())] },
                tree: TreeSelector {
                    node: vec![Segment::ExactMatch("bar".into())],
                    property: None,
                    tree_names: None
                },
            }
        );

        // parses tree names
        assert_eq!(
            selector::<VerboseError>(r#"core/**:[name=foo, name="bar\*"]some-node/he*re:prop"#)
                .unwrap(),
            Selector {
                component: ComponentSelector {
                    segments: vec![
                        Segment::ExactMatch("core".into()),
                        Segment::Pattern("**".into()),
                    ],
                },
                tree: TreeSelector {
                    node: vec![
                        Segment::ExactMatch("some-node".into()),
                        Segment::Pattern("he*re".into()),
                    ],
                    property: Some(Segment::ExactMatch("prop".into())),
                    tree_names: Some(vec!["foo", r"bar*"].into()),
                },
            }
        );

        assert_eq!(
            selector::<VerboseError>(r#"core/**:[name="foo:bar"]some-node/he*re:prop"#).unwrap(),
            Selector {
                component: ComponentSelector {
                    segments: vec![
                        Segment::ExactMatch("core".into()),
                        Segment::Pattern("**".into()),
                    ],
                },
                tree: TreeSelector {
                    node: vec![
                        Segment::ExactMatch("some-node".into()),
                        Segment::Pattern("he*re".into()),
                    ],
                    property: Some(Segment::ExactMatch("prop".into())),
                    tree_names: Some(vec!["foo:bar"].into()),
                },
            }
        );

        assert_eq!(
            selector::<VerboseError>(r#"core/**:[name="name=bar"]some-node/he*re:prop"#).unwrap(),
            Selector {
                component: ComponentSelector {
                    segments: vec![
                        Segment::ExactMatch("core".into()),
                        Segment::Pattern("**".into()),
                    ],
                },
                tree: TreeSelector {
                    node: vec![
                        Segment::ExactMatch("some-node".into()),
                        Segment::Pattern("he*re".into()),
                    ],
                    property: Some(Segment::ExactMatch("prop".into())),
                    tree_names: Some(vec!["name=bar"].into()),
                },
            }
        );

        assert_eq!(
            selector::<VerboseError>(r#"core/**:[name=foo-bar_baz]some-node/he*re:prop"#).unwrap(),
            Selector {
                component: ComponentSelector {
                    segments: vec![
                        Segment::ExactMatch("core".into()),
                        Segment::Pattern("**".into()),
                    ],
                },
                tree: TreeSelector {
                    node: vec![
                        Segment::ExactMatch("some-node".into()),
                        Segment::Pattern("he*re".into()),
                    ],
                    property: Some(Segment::ExactMatch("prop".into())),
                    tree_names: Some(vec!["foo-bar_baz"].into()),
                },
            }
        );

        // At least one filter is required when `where` is provided.
        assert!(selector::<VerboseError>("foo:bar where").is_err());
    }

    #[fuchsia::test]
    fn assert_no_trailing_backward_slash() {
        assert!(selector::<VerboseError>(r#"foo:bar:baz\"#).is_err());
    }

    #[fuchsia::test]
    fn parse_full_selector_with_spaces() {
        let expected_regardless_of_escape_or_quote = Selector {
            component: ComponentSelector {
                segments: vec![
                    Segment::ExactMatch("core".into()),
                    Segment::ExactMatch("foo".into()),
                ],
            },
            tree: TreeSelector {
                node: vec![Segment::ExactMatch("some node".into()), Segment::Pattern("*".into())],
                property: Some(Segment::ExactMatch("prop".into())),
                tree_names: None,
            },
        };
        assert_eq!(
            selector::<VerboseError>(r#"core/foo:some\ node/*:prop"#).unwrap(),
            expected_regardless_of_escape_or_quote,
        );

        assert_eq!(
            selector::<VerboseError>(r#""core/foo:some node/*:prop""#).unwrap(),
            expected_regardless_of_escape_or_quote,
        );
    }

    #[fuchsia::test]
    fn unclosed_quotes_rejected() {
        assert!(selector::<VerboseError>(r#""core/foo:some node/*:prop"#).is_err());
        assert!(selector::<VerboseError>(r#""core/foo:some node/*:propx"#).is_err());
        assert!(selector::<VerboseError>(r#""a:b:cde"#).is_err());
        assert!(selector::<VerboseError>(r#""core/foo:root:bar🦀"#).is_err());
    }

    #[fuchsia::test]
    fn test_extract_from_quotes() {
        let test_cases = [
            ("foo", "foo"),
            (r#""foo""#, "foo"),
            (r#""foo\"bar""#, r#"foo\"bar"#),
            (r#""bar\*""#, r#"bar\*"#),
            (r#""foo"#, r#""foo"#),
            (r#""a"#, r#""a"#),
            (r#""foo\"bar"#, r#""foo\"bar"#),
            (r#""foo"bar"#, r#""foo"bar"#),
            (r#""🦀"#, r#""🦀"#),
            (r#"""#, r#"""#),
            (r#""""#, ""),
        ];

        for (case_number, (input, expected_extracted)) in test_cases.into_iter().enumerate() {
            let actual_extracted = extract_from_quotes(input);
            assert_eq!(
                expected_extracted, actual_extracted,
                "failed test case {case_number} on name_list: |{input}|",
            );
        }
    }

    #[fuchsia::test]
    fn extract_name_list() {
        let test_cases = [
            ("root:prop", ("root:prop", None)),
            ("[name=foo]root:prop", ("root:prop", Some(TreeNames::from(vec!["foo"])))),
            (
                r#"[name="with internal ,"]root"#,
                ("root", Some(TreeNames::from(vec!["with internal ,"]))),
            ),
            (r#"[name="f[o]o"]root:prop"#, ("root:prop", Some(TreeNames::from(vec!["f[o]o"])))),
            (
                r#"[name="fo]o", name="[bar,baz"]root:prop"#,
                ("root:prop", Some(TreeNames::from(vec!["fo]o", "[bar,baz"]))),
            ),
            (r#"ab/\ d:c\ "#, (r#"ab/\ d:c\ "#, None)),
        ];

        for (case_number, (input, (expected_residue, expected_name_list))) in
            test_cases.into_iter().enumerate()
        {
            let mut i = input;
            let actual_name_list =
                conjoined_tree_names::<winnow::error::ContextError>().parse_next(&mut i).unwrap();
            let actual_residue = i;
            assert_eq!(
                expected_residue, actual_residue,
                "failed test case {case_number} on residue: |{input}|",
            );
            assert_eq!(
                expected_name_list, actual_name_list,
                "failed test case {case_number} on name_list: |{input}|",
            );
        }
    }

    #[fuchsia::test]
    fn comma_separated_name_lists() {
        let test_cases = [
            (r#"name=foo, name=bar"#, vec!["foo", "bar"]),
            (r#"name="with internal ,""#, vec!["with internal ,"]),
            (r#"name="foo", name=bar"#, vec!["foo", "bar"]),
            (r#"name="foo,bar", name=baz"#, vec!["foo,bar", "baz"]),
            (r#"name="foo,bar", name="baz""#, vec!["foo,bar", "baz"]),
            (r#"name="foo ,bar", name=baz"#, vec!["foo ,bar", "baz"]),
            (r#"name="foo\",bar", name="baz""#, vec![r#"foo\",bar"#, "baz"]),
            (r#"name="foo\" ,bar", name="baz""#, vec![r#"foo\" ,bar"#, "baz"]),
            (r#"name="foo,bar", name=" baz  ""#, vec!["foo,bar", " baz  "]),
            (r#"name="foo\", bar,", name=",,baz,,,""#, vec![r#"foo\", bar,"#, ",,baz,,,"]),
        ];

        for (case_number, (input, expected)) in test_cases.into_iter().enumerate() {
            let mut i = input;
            let actual: Vec<&str> =
                separated(1.., tree_name_item::<winnow::error::ContextError>, spaced(","))
                    .parse_next(&mut i)
                    .unwrap();
            assert_eq!(expected, actual, "failed test case {case_number} on list: |{input}|",);
        }
    }
}
