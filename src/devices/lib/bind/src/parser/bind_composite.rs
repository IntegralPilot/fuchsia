// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::parser::bind_rules::{Statement, StatementBlock, statement_block};
use crate::parser::common::{
    BindParserError, CompoundIdentifier, Include, NomSpan, ParentType, compound_identifier,
    many_until_eof, map_err, using_list, ws,
};
use nom::branch::alt;
use nom::bytes::complete::{escaped, is_not, tag};
use nom::character::complete::{char, one_of};
use nom::combinator::{map, opt};
use nom::sequence::delimited;
use nom::{IResult, Parser};
use std::collections::HashSet;

#[derive(Debug, PartialEq)]
pub struct Parent<'a> {
    pub name: String,
    pub statements: StatementBlock<'a>,
}

#[derive(Debug, PartialEq)]
pub struct Ast<'a> {
    pub name: CompoundIdentifier,
    pub using: Vec<Include>,
    pub primary_parent: Parent<'a>,
    pub additional_parents: Vec<Parent<'a>>,
    pub optional_parents: Vec<Parent<'a>>,
}

impl<'a> TryFrom<&'a str> for Ast<'a> {
    type Error = BindParserError;

    fn try_from(input: &'a str) -> Result<Self, Self::Error> {
        match composite(NomSpan::new(input)) {
            Ok((_, ast)) => Ok(ast),
            Err(nom::Err::Error(e)) => Err(e),
            Err(nom::Err::Failure(e)) => Err(e),
            Err(nom::Err::Incomplete(_)) => {
                unreachable!("Parser should never generate Incomplete errors")
            }
        }
    }
}

fn keyword_composite(input: NomSpan) -> IResult<NomSpan, NomSpan, BindParserError> {
    ws(map_err(tag("composite"), BindParserError::CompositeKeyword)).parse(input)
}

fn keyword_parent(input: NomSpan) -> IResult<NomSpan, NomSpan, BindParserError> {
    let (input, kw) =
        ws(map_err(alt((tag("node"), tag("parent"))), BindParserError::ParentKeyword))
            .parse(input)?;
    if kw.fragment() == &"node" {
        return Err(nom::Err::Error(BindParserError::DeprecatedNodeKeyword(
            kw.fragment().to_string(),
        )));
    }
    Ok((input, kw))
}

fn keyword_primary(input: NomSpan) -> IResult<NomSpan, NomSpan, BindParserError> {
    ws(map_err(tag("primary"), BindParserError::PrimaryOrOptionalKeyword)).parse(input)
}

fn keyword_optional(input: NomSpan) -> IResult<NomSpan, NomSpan, BindParserError> {
    ws(map_err(tag("optional"), BindParserError::PrimaryOrOptionalKeyword)).parse(input)
}

fn parent_type(input: NomSpan) -> IResult<NomSpan, ParentType, BindParserError> {
    let (input, keyword) = opt(alt((keyword_optional, keyword_primary))).parse(input)?;
    match keyword {
        Some(kw) => match kw.fragment() {
            &"optional" => Ok((input, ParentType::Optional)),
            &"primary" => Ok((input, ParentType::Primary)),
            &&_ => Err(nom::Err::Error(BindParserError::PrimaryOrOptionalKeyword(
                kw.fragment().to_string(),
            ))),
        },
        None => Ok((input, ParentType::Additional)),
    }
}

fn composite_name(input: NomSpan) -> IResult<NomSpan, CompoundIdentifier, BindParserError> {
    let terminator = ws(map_err(tag(";"), BindParserError::Semicolon));
    delimited(keyword_composite, ws(compound_identifier), terminator).parse(input)
}

fn parent_name(input: NomSpan) -> IResult<NomSpan, String, BindParserError> {
    let escapable = escaped(is_not(r#"\""#), '\\', one_of(r#"\""#));
    let literal = delimited(char('"'), escapable, char('"'));
    map_err(map(literal, |s: NomSpan| s.fragment().to_string()), BindParserError::InvalidParentName)
        .parse(input)
}

fn parent(
    input: NomSpan,
) -> IResult<NomSpan, (ParentType, String, Vec<Statement>), BindParserError> {
    let (input, parent_type) = parent_type(input)?;
    let (input, _parent) = keyword_parent(input)?;
    let (input, parent_name) = ws(parent_name).parse(input)?;

    let (input, statements) = statement_block(input)?;
    return Ok((input, (parent_type, parent_name, statements)));
}

fn composite<'a>(input: NomSpan<'a>) -> IResult<NomSpan, Ast, BindParserError> {
    let parents = |input: NomSpan<'a>| -> IResult<
        NomSpan,
        (Parent<'a>, Vec<Parent<'a>>, Vec<Parent<'a>>),
        BindParserError,
    > {
        let (input, parents) = many_until_eof(ws(parent)).parse(input)?;
        if parents.is_empty() {
            return Err(nom::Err::Error(BindParserError::NoParents(input.to_string())));
        }
        let mut primary_parent = None;
        let mut additional_parents = vec![];
        let mut optional_parents = vec![];
        let mut parent_names = HashSet::new();
        for (parent_type, name, statements) in parents {
            if parent_names.contains(&name) {
                return Err(nom::Err::Error(BindParserError::DuplicateParentName(
                    input.to_string(),
                )));
            }
            parent_names.insert(name.clone());

            match parent_type {
                ParentType::Primary => {
                    if primary_parent.is_some() {
                        return Err(nom::Err::Error(BindParserError::OnePrimaryParent(
                            input.to_string(),
                        )));
                    }
                    primary_parent = Some(Parent { name: name, statements: statements });
                }
                ParentType::Additional => {
                    additional_parents.push(Parent { name: name, statements: statements });
                }
                ParentType::Optional => {
                    optional_parents.push(Parent { name: name, statements: statements });
                }
            }
        }
        if let Some(primary_parent) = primary_parent {
            return Ok((input, (primary_parent, additional_parents, optional_parents)));
        }
        return Err(nom::Err::Error(BindParserError::OnePrimaryParent(input.to_string())));
    };
    map(
        (ws(composite_name), ws(using_list), parents),
        |(name, using, (primary_parent, additional_parents, optional_parents))| Ast {
            name,
            using,
            primary_parent,
            additional_parents,
            optional_parents,
        },
    )
    .parse(input)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::make_identifier;
    use crate::parser::bind_rules::{Condition, ConditionOp};
    use crate::parser::common::test::check_result;
    use crate::parser::common::{Span, Value};

    mod composite_name {
        use super::*;

        #[test]
        fn single_name() {
            check_result(composite_name(NomSpan::new("composite a;")), "", make_identifier!["a"]);
        }

        #[test]
        fn compound_name() {
            check_result(
                composite_name(NomSpan::new("composite a.b;")),
                "",
                make_identifier!["a", "b"],
            );
        }

        #[test]
        fn whitespace() {
            check_result(
                composite_name(NomSpan::new("composite \n\t a\n\t ;")),
                "",
                make_identifier!["a"],
            );
        }

        #[test]
        fn invalid() {
            // Must have a name.
            assert_eq!(
                composite_name(NomSpan::new("composite ;")),
                Err(nom::Err::Error(BindParserError::Identifier(";".to_string())))
            );

            // Must be terminated by ';'.
            assert_eq!(
                composite_name(NomSpan::new("composite a")),
                Err(nom::Err::Error(BindParserError::Semicolon("".to_string())))
            );
        }

        #[test]
        fn empty() {
            // Does not match empty string.
            assert_eq!(
                composite_name(NomSpan::new("")),
                Err(nom::Err::Error(BindParserError::CompositeKeyword("".to_string())))
            );
        }
    }

    mod composites {
        use super::*;

        #[test]
        fn empty() {
            // Does not match empty string.
            assert_eq!(
                composite(NomSpan::new("")),
                Err(nom::Err::Error(BindParserError::CompositeKeyword("".to_string())))
            );
        }

        #[test]
        fn node_keyword_error() {
            assert_eq!(
                composite(NomSpan::new("composite a; primary node \"bananaquit\" { true; }")),
                Err(nom::Err::Error(BindParserError::DeprecatedNodeKeyword("node".to_string())))
            );
        }

        #[test]
        fn one_primary_parent() {
            check_result(
                composite(NomSpan::new("composite a; primary parent \"bananaquit\" { true; }")),
                "",
                Ast {
                    name: make_identifier!["a"],
                    using: vec![],
                    primary_parent: Parent {
                        name: "bananaquit".to_string(),
                        statements: vec![Statement::True {
                            span: Span { offset: 43, line: 1, fragment: "true;" },
                        }],
                    },
                    additional_parents: vec![],
                    optional_parents: vec![],
                },
            );
        }

        #[test]
        fn one_primary_parent_keyword() {
            check_result(
                composite(NomSpan::new("composite a; primary parent \"pdev\" { true; }")),
                "",
                Ast {
                    name: make_identifier!["a"],
                    using: vec![],
                    primary_parent: Parent {
                        name: "pdev".to_string(),
                        statements: vec![Statement::True {
                            span: Span { offset: 37, line: 1, fragment: "true;" },
                        }],
                    },
                    additional_parents: vec![],
                    optional_parents: vec![],
                },
            );
        }

        #[test]
        fn one_primary_parent_one_additional() {
            check_result(
                composite(NomSpan::new(
                    "composite a; primary parent \"dipper\" { true; } parent \"streamcreeper\" { false; }",
                )),
                "",
                Ast {
                    name: make_identifier!["a"],
                    using: vec![],
                    primary_parent: Parent {
                        name: "dipper".to_string(),
                        statements: vec![Statement::True {
                            span: Span { offset: 39, line: 1, fragment: "true;" },
                        }],
                    },
                    additional_parents: vec![Parent {
                        name: "streamcreeper".to_string(),
                        statements: vec![Statement::False {
                            span: Span { offset: 72, line: 1, fragment: "false;" },
                        }],
                    }],
                    optional_parents: vec![],
                },
            );
        }

        #[test]
        fn one_primary_parent_one_optional() {
            check_result(
                composite(NomSpan::new(
                    "composite a; primary parent \"dipper\" { true; } optional parent \"oilbird\" { x == 1; }",
                )),
                "",
                Ast {
                    name: make_identifier!["a"],
                    using: vec![],
                    primary_parent: Parent {
                        name: "dipper".to_string(),
                        statements: vec![Statement::True {
                            span: Span { offset: 39, line: 1, fragment: "true;" },
                        }],
                    },
                    additional_parents: vec![],
                    optional_parents: vec![Parent {
                        name: "oilbird".to_string(),
                        statements: vec![Statement::ConditionStatement {
                            span: Span { offset: 75, line: 1, fragment: "x == 1;" },
                            condition: Condition {
                                span: Span { offset: 75, line: 1, fragment: "x == 1" },
                                lhs: make_identifier!["x"],
                                op: ConditionOp::Equals,
                                rhs: Value::NumericLiteral(1),
                            },
                        }],
                    }],
                },
            );
        }

        #[test]
        fn one_primary_parent_one_additional_one_optional() {
            check_result(
                composite(NomSpan::new(
                    "composite a; primary parent \"dipper\" { true; } parent \"streamcreeper\" { false; } optional parent \"oilbird\" { x == 1; }",
                )),
                "",
                Ast {
                    name: make_identifier!["a"],
                    using: vec![],
                    primary_parent: Parent {
                        name: "dipper".to_string(),
                        statements: vec![Statement::True {
                            span: Span { offset: 39, line: 1, fragment: "true;" },
                        }],
                    },
                    additional_parents: vec![Parent {
                        name: "streamcreeper".to_string(),
                        statements: vec![Statement::False {
                            span: Span { offset: 72, line: 1, fragment: "false;" },
                        }],
                    }],
                    optional_parents: vec![Parent {
                        name: "oilbird".to_string(),
                        statements: vec![Statement::ConditionStatement {
                            span: Span { offset: 109, line: 1, fragment: "x == 1;" },
                            condition: Condition {
                                span: Span { offset: 109, line: 1, fragment: "x == 1" },
                                lhs: make_identifier!["x"],
                                op: ConditionOp::Equals,
                                rhs: Value::NumericLiteral(1),
                            },
                        }],
                    }],
                },
            );
        }

        #[test]
        fn one_primary_parent_two_additional() {
            check_result(
                composite(NomSpan::new(
                    "composite a; primary parent \"fireback\" { true; } parent \"ovenbird\" { false; } parent \"oilbird\" { x == 1; }",
                )),
                "",
                Ast {
                    name: make_identifier!["a"],
                    using: vec![],
                    primary_parent: Parent {
                        name: "fireback".to_string(),
                        statements: vec![Statement::True {
                            span: Span { offset: 41, line: 1, fragment: "true;" },
                        }],
                    },
                    additional_parents: vec![
                        Parent {
                            name: "ovenbird".to_string(),
                            statements: vec![Statement::False {
                                span: Span { offset: 69, line: 1, fragment: "false;" },
                            }],
                        },
                        Parent {
                            name: "oilbird".to_string(),
                            statements: vec![Statement::ConditionStatement {
                                span: Span { offset: 97, line: 1, fragment: "x == 1;" },
                                condition: Condition {
                                    span: Span { offset: 97, line: 1, fragment: "x == 1" },
                                    lhs: make_identifier!["x"],
                                    op: ConditionOp::Equals,
                                    rhs: Value::NumericLiteral(1),
                                },
                            }],
                        },
                    ],
                    optional_parents: vec![],
                },
            );
        }

        #[test]
        fn using_list() {
            check_result(
                composite(NomSpan::new(
                    "composite a; using x.y as z; primary parent \"oilbird\" { true; }",
                )),
                "",
                Ast {
                    name: make_identifier!["a"],
                    using: vec![Include {
                        name: make_identifier!["x", "y"],
                        alias: Some("z".to_string()),
                    }],
                    primary_parent: Parent {
                        name: "oilbird".to_string(),
                        statements: vec![Statement::True {
                            span: Span { offset: 56, line: 1, fragment: "true;" },
                        }],
                    },
                    additional_parents: vec![],
                    optional_parents: vec![],
                },
            );
        }

        #[test]
        fn no_nodes() {
            assert_eq!(
                composite(NomSpan::new("composite a; using x.y as z;")),
                Err(nom::Err::Error(BindParserError::NoParents("".to_string())))
            );
            assert_eq!(
                composite(NomSpan::new("composite a;")),
                Err(nom::Err::Error(BindParserError::NoParents("".to_string())))
            );
        }

        #[test]
        fn not_one_primary_parent() {
            assert_eq!(
                composite(NomSpan::new("composite a; parent \"chiffchaff\"{ true; }")),
                Err(nom::Err::Error(BindParserError::OnePrimaryParent("".to_string())))
            );
            assert_eq!(
                composite(NomSpan::new(
                    "composite a; primary parent \"chiffchaff\" { true; } primary parent \"warbler\" { false; }"
                )),
                Err(nom::Err::Error(BindParserError::OnePrimaryParent("".to_string())))
            );
        }

        #[test]
        fn no_primary_parent_name() {
            assert_eq!(
                composite(NomSpan::new("composite a; primary parent { true; }")),
                Err(nom::Err::Error(BindParserError::InvalidParentName("{ true; }".to_string())))
            );
            assert_eq!(
                composite(NomSpan::new("composite a; primary parent chiffchaff { true; }")),
                Err(nom::Err::Error(BindParserError::InvalidParentName(
                    "chiffchaff { true; }".to_string()
                )))
            );

            assert_eq!(
                composite(NomSpan::new("composite a; primary parent chiffchaff\" { true; }")),
                Err(nom::Err::Error(BindParserError::InvalidParentName(
                    "chiffchaff\" { true; }".to_string()
                )))
            );
        }

        #[test]
        fn no_parent_name() {
            assert_eq!(
                composite(NomSpan::new(
                    "composite a; primary parent \"oilbird\" { true; } parent { x == 1; }"
                )),
                Err(nom::Err::Error(BindParserError::InvalidParentName("{ x == 1; }".to_string())))
            );
            assert_eq!(
                composite(NomSpan::new(
                    "composite a; primary parent \"oilbird\" { true; } parent \"warbler { x == 1; }"
                )),
                Err(nom::Err::Error(BindParserError::InvalidParentName("".to_string())))
            );

            assert_eq!(
                composite(NomSpan::new(
                    "composite a; primary parent \"oilbird\" { true; } parent warbler { x == 1; }"
                )),
                Err(nom::Err::Error(BindParserError::InvalidParentName(
                    "warbler { x == 1; }".to_string()
                )))
            );
        }

        #[test]
        fn duplicate_parent_names() {
            assert_eq!(
                composite(NomSpan::new(
                    "composite a; primary parent \"bobolink\" { true; } parent \"bobolink\" { x == 1; }"
                )),
                Err(nom::Err::Error(BindParserError::DuplicateParentName("".to_string())))
            );

            assert_eq!(
                composite(NomSpan::new(
                    "composite a; primary parent \"bobolink\" { true; } parent \"cowbird\" { x == 1; } parent \"cowbird\" { false; }"
                )),
                Err(nom::Err::Error(BindParserError::DuplicateParentName("".to_string())))
            );
        }
    }
}
