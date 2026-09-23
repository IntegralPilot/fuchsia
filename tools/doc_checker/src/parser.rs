// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! parser handles parsing markdown via pulldown_cmark into higher level elements.

use pulldown_cmark::Event::{
    self, Code, DisplayMath, End, FootnoteReference, HardBreak, Html, InlineHtml, InlineMath,
    SoftBreak, Start, TaskListMarker, Text,
};
use pulldown_cmark::{CodeBlockKind, CowStr, Tag, TagEnd};
use std::ops::Range;

use crate::md_element::{DocContext, Element};

pub(crate) fn element_from_event<'a>(
    event: Event<'a>,
    range: Range<usize>,
    doc_context: &mut DocContext<'a>,
) -> Element<'a> {
    let span = doc_context.span(&range);
    let newlines = span.chars().filter(|c| c == &'\n').count();
    let element = match event {
        // Start indicates the start of a Tag.
        Start(Tag::Heading { .. }) => {
            let mut element = parse_tag_element(event, doc_context);
            // update the line number in doc_context, returning the line_number of the element.
            element.set_line_num(update_line_number(doc_context, span, newlines));
            element
        }
        Start(Tag::Paragraph) => {
            let element = parse_tag_element(event, doc_context);
            doc_context.line_num += newlines;
            element
        }
        Start(_) => parse_tag_element(event, doc_context),
        End(_) => {
            //should not happen
            panic!("Got End event unexpectedly: {:?}", event)
        }
        Text(text) => {
            let mut element = Element::Text(text, doc_context.line());
            // update the line number in doc_context, returning the line_number of the element.
            element.set_line_num(update_line_number(doc_context, span, newlines));
            element
        }
        Code(text) => {
            let mut element = Element::Code(text, doc_context.line());
            // update the line number in doc_context, returning the line_number of the element.
            element.set_line_num(update_line_number(doc_context, span, newlines));
            element
        }
        Html(text) => {
            let mut element = Element::Html(text, doc_context.line());
            // update the line number in doc_context, returning the line_number of the element.
            element.set_line_num(update_line_number(doc_context, span, newlines));
            element
        }
        FootnoteReference(text) => Element::FootnoteReference(text, doc_context.line()),
        HardBreak => {
            let element = Element::HardBreak(doc_context.line());
            doc_context.line_num += newlines;
            element
        }
        SoftBreak => {
            let element = Element::SoftBreak(doc_context.line());
            // Don't increment line_num here, it is handled in the Text event.
            element
        }
        Event::Rule => {
            let element = Element::Rule(doc_context.line());

            doc_context.line_num += newlines;
            element
        }
        TaskListMarker(checked) => Element::TaskListMarker(checked, doc_context.line()),
        InlineMath(text) | DisplayMath(text) => {
            let mut element = Element::Text(text, doc_context.line());
            element.set_line_num(update_line_number(doc_context, span, newlines));
            element
        }
        InlineHtml(text) => {
            let mut element = Element::Html(text, doc_context.line());
            element.set_line_num(update_line_number(doc_context, span, newlines));
            element
        }
    };
    element
}

fn update_line_number<'a>(doc_context: &mut DocContext<'a>, span: &str, newlines: usize) -> usize {
    let current_line_num = doc_context.line_num;
    if let Some(line_num) = doc_context.line_number_of(span) {
        doc_context.line_num = line_num + 1;
        return line_num;
    } else {
        doc_context.line_num += newlines;
    }
    current_line_num
}

fn parse_tag_element<'a>(event: Event<'a>, doc_context: &mut DocContext<'a>) -> Element<'a> {
    match event {
        Start(Tag::Paragraph) => {
            let block = read_block(Tag::Paragraph, doc_context);
            doc_context.line_num += 1;
            block
        }
        Start(Tag::Heading { level, id, classes, attrs }) => {
            let block = read_block(Tag::Heading { level, id, classes, attrs }, doc_context);

            doc_context.line_num += 1;
            block
        }
        Start(Tag::BlockQuote(kind)) => {
            let block = read_block(Tag::BlockQuote(kind), doc_context);
            block
        }
        Start(Tag::CodeBlock(CodeBlockKind::Fenced(code))) => {
            let block = read_codeblock(code, doc_context);
            block
        }
        Start(Tag::CodeBlock(CodeBlockKind::Indented)) => {
            let block = read_block(Tag::CodeBlock(CodeBlockKind::Indented), doc_context);
            block
        }
        Start(Tag::List(starting)) => {
            let block = read_list(starting, doc_context);
            block
        }
        Start(Tag::Item) => {
            let block = read_block(Tag::Item, doc_context);
            doc_context.line_num += 1;
            block
        }
        Start(Tag::FootnoteDefinition(text)) => {
            read_block(Tag::FootnoteDefinition(text), doc_context)
        }
        Start(Tag::Table(alignment)) => read_block(Tag::Table(alignment), doc_context),
        Start(Tag::TableCell) => read_block(Tag::TableCell, doc_context),
        Start(Tag::TableHead) => read_block(Tag::TableHead, doc_context),
        Start(Tag::TableRow) => read_block(Tag::TableRow, doc_context),
        Start(Tag::Emphasis) => read_block(Tag::Emphasis, doc_context),
        Start(Tag::Strong) => read_block(Tag::Strong, doc_context),
        Start(Tag::Strikethrough) => read_block(Tag::Strikethrough, doc_context),
        Start(Tag::Link { link_type, dest_url, title, id: _ }) => {
            read_link(link_type, dest_url, title, doc_context)
        }
        Start(Tag::Image { link_type, dest_url, title, id: _ }) => {
            read_image(link_type, dest_url, title, doc_context)
        }
        Start(tag) => read_block(tag, doc_context),
        End(tag) => panic!("Unexpected tag: {:?}", tag),
        Text(text) => {
            let element = Element::Text(text, doc_context.line());
            element
        }
        Code(text) => Element::Code(text, doc_context.line()),
        Html(text) => Element::Html(text, doc_context.line()),
        FootnoteReference(text) => Element::FootnoteReference(text, doc_context.line()),
        HardBreak => Element::HardBreak(doc_context.line()),
        SoftBreak => Element::SoftBreak(doc_context.line()),
        Event::Rule => Element::Rule(doc_context.line()),
        TaskListMarker(checked) => Element::TaskListMarker(checked, doc_context.line()),
        InlineMath(text) | DisplayMath(text) => Element::Text(text, doc_context.line()),
        InlineHtml(text) => Element::Html(text, doc_context.line()),
    }
}

fn read_image<'a>(
    link_type: pulldown_cmark::LinkType,
    link_url: pulldown_cmark::CowStr<'a>,
    title: pulldown_cmark::CowStr<'a>,
    doc_context: &mut DocContext<'a>,
) -> Element<'a> {
    let mut contents = vec![];
    while let Some((event, range)) = doc_context.parser.next() {
        match event {
            End(TagEnd::Image) => {
                return Element::Image(link_type, link_url, title, contents, doc_context.line());
            }
            _ => contents.push(element_from_event(event, range, doc_context)),
        };
    }
    // Using :? since link_type does not implement Display.
    // This should not happen, so panic is the way to go.
    panic!("{:?} {} {} has no end?", link_type, link_url, title);
}

fn read_link<'a>(
    link_type: pulldown_cmark::LinkType,
    link_url: pulldown_cmark::CowStr<'a>,
    title: pulldown_cmark::CowStr<'a>,
    doc_context: &mut DocContext<'a>,
) -> Element<'a> {
    let mut contents = vec![];
    // Using loop here vs. while let  so that it is easy to
    // catch the situation where the End tag is not found
    while let Some((event, range)) = doc_context.parser.next() {
        match event {
            End(TagEnd::Link) => {
                return Element::Link(link_type, link_url, title, contents, doc_context.line());
            }
            _ => contents.push(element_from_event(event, range, doc_context)),
        };
    }

    // This should not happen, so not sure how to handle it happening?
    panic!("{:?} {} {} has no end?", link_type, link_url, title);
}

fn read_list<'a>(starting: Option<u64>, doc_context: &mut DocContext<'a>) -> Element<'a> {
    let mut items = vec![];
    let start = doc_context.line();
    while let Some((event, range)) = doc_context.parser.next() {
        match event {
            End(TagEnd::List(_)) => return Element::List(starting, items, start),
            _ => items.push(element_from_event(event, range, doc_context)),
        };
    }
    // This should not happen, so panic.
    panic!("{:?} has no end?", starting);
}

fn read_codeblock<'a>(code: CowStr<'a>, doc_context: &mut DocContext<'a>) -> Element<'a> {
    let mut elements = vec![];
    let start = doc_context.line();
    while let Some((event, range)) = doc_context.parser.next() {
        match event {
            End(TagEnd::CodeBlock) => return Element::CodeBlock(code, elements, start),
            _ => elements.push(element_from_event(event, range, doc_context)),
        };
    }
    // This should not happen, so panic.
    panic!("{:?} has no end?", code);
}

fn read_block<'a>(block_type: Tag<'a>, doc_context: &mut DocContext<'a>) -> Element<'a> {
    let mut elements = vec![];
    let start = doc_context.line();
    let end_tag = block_type.to_end();
    while let Some((event, range)) = doc_context.parser.next() {
        match event {
            End(tag) if tag == end_tag => {
                let block = Element::Block(block_type, elements, start);
                return block;
            }
            _ => elements.push(element_from_event(event, range, doc_context)),
        };
    }
    // This should not happen, so panic.
    panic!("{:?} has no end?", block_type);
}

#[cfg(test)]
mod test {

    use super::*;
    use crate::DocLine;
    use pulldown_cmark::CowStr::Borrowed;
    use pulldown_cmark::{HeadingLevel, LinkType};
    use std::path::PathBuf;

    #[test]
    fn test_basic_elements() {
        let doc_line1 = DocLine { line_num: 1, file_name: PathBuf::from("test1") };
        let doc_line2 = DocLine { line_num: 2, file_name: PathBuf::from("test1") };
        let test_data = [
            (
                "This is a string",
                vec![Element::Block(
                    Tag::Paragraph,
                    vec![Element::Text(Borrowed("This is a string"), doc_line1.clone())],
                    doc_line1.clone(),
                )],
            ),
            (
                "`This is code`",
                vec![Element::Block(
                    Tag::Paragraph,
                    vec![Element::Code(Borrowed("This is code"), doc_line1.clone())],
                    doc_line1.clone(),
                )],
            ),
            (
                "<b>",
                vec![Element::Block(
                    Tag::HtmlBlock,
                    vec![Element::Html(Borrowed("<b>"), doc_line1.clone())],
                    doc_line1.clone(),
                )],
            ),
            (
                "# Header 1",
                vec![Element::Block(
                    Tag::Heading {
                        level: HeadingLevel::H1,
                        id: None,
                        classes: vec![],
                        attrs: vec![],
                    },
                    vec![Element::Text(Borrowed("Header 1"), doc_line1.clone())],
                    doc_line1.clone(),
                )],
            ),
            (
                r##"# Header 1
Multiline
"##,
                vec![
                    Element::Block(
                        Tag::Heading {
                            level: HeadingLevel::H1,
                            id: None,
                            classes: vec![],
                            attrs: vec![],
                        },
                        vec![Element::Text(Borrowed("Header 1"), doc_line1.clone())],
                        doc_line1.clone(),
                    ),
                    Element::Block(
                        Tag::Paragraph,
                        vec![Element::Text(Borrowed("Multiline"), doc_line2.clone())],
                        doc_line2,
                    ),
                ],
            ),
            (
                "[link text](http://to.my/webpage)",
                vec![Element::Block(
                    Tag::Paragraph,
                    vec![Element::Link(
                        LinkType::Inline,
                        Borrowed("http://to.my/webpage"),
                        Borrowed(""),
                        vec![Element::Text(Borrowed("link text"), doc_line1.clone())],
                        doc_line1.clone(),
                    )],
                    doc_line1,
                )],
            ),
        ];

        for (input, expected) in test_data {
            let callback = &mut |broken_link: pulldown_cmark::BrokenLink<'_>| {
                DocContext::handle_broken_link(broken_link, input)
            };

            let ctx = DocContext::new(PathBuf::from("test1"), input, Some(callback));
            let actual: Vec<Element<'_>> = ctx.collect();
            assert_eq!(actual.len(), expected.len());
            let mut expected_iter = expected.iter();
            for a in actual {
                if let Some(b) = expected_iter.next() {
                    assert_eq!(&a, b);
                }
            }
        }
    }
}
