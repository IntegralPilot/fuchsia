// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::doctor_ledger::*;
use std::fmt;
use termio::Colors;

const INDENT_STR: &str = "    ";

pub trait LedgerView: fmt::Display {
    fn set(&mut self, new_tree: LedgerViewNode);
}

pub enum LedgerViewOutcome {
    Info,
    Success,
    Warning,
    Failure,
    Invalid,
}

impl From<LedgerOutcome> for LedgerViewOutcome {
    fn from(view: LedgerOutcome) -> LedgerViewOutcome {
        match view {
            LedgerOutcome::Info => LedgerViewOutcome::Info,
            LedgerOutcome::Success => LedgerViewOutcome::Success,
            LedgerOutcome::Warning => LedgerViewOutcome::Warning,
            LedgerOutcome::SoftWarning => LedgerViewOutcome::Warning,
            LedgerOutcome::Failure => LedgerViewOutcome::Failure,
            _ => LedgerViewOutcome::Invalid,
        }
    }
}

impl LedgerViewOutcome {
    pub fn format(&self, colors: &Colors) -> String {
        let (symbol, color) = match self {
            LedgerViewOutcome::Info => ("i", colors.green),
            LedgerViewOutcome::Success => ("✓", colors.green),
            LedgerViewOutcome::Warning => ("!", colors.yellow),
            LedgerViewOutcome::Failure => ("✗", colors.red),
            LedgerViewOutcome::Invalid => (" ", colors.red),
        };
        format!("{}{}{}", color, symbol, colors.reset)
    }
}

pub struct LedgerViewNode {
    pub data: String,
    pub outcome: LedgerViewOutcome,
    pub children: Vec<LedgerViewNode>,
}

impl Default for LedgerViewNode {
    fn default() -> LedgerViewNode {
        LedgerViewNode {
            data: "".to_string(),
            outcome: LedgerViewOutcome::Invalid,
            children: vec![],
        }
    }
}

fn gen_output(parent_node: &LedgerViewNode, colors: &Colors, indent_level: usize) -> String {
    let mut output_str = format!(
        "{}[{}] {}\n",
        INDENT_STR.repeat(indent_level),
        parent_node.outcome.format(colors),
        parent_node.data
    );

    for child_node in &parent_node.children {
        let child_str = gen_output(child_node, colors, indent_level + 1);
        output_str = format!("{}{}", output_str, child_str);
    }

    return output_str;
}

// Default LedgerView.
pub struct VisualLedgerView {
    tree: LedgerViewNode,
}

impl fmt::Display for VisualLedgerView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", gen_output(&self.tree, &Colors::current(), 0))
    }
}

impl VisualLedgerView {
    pub fn new() -> Self {
        VisualLedgerView { tree: LedgerViewNode::default() }
    }
}

impl LedgerView for VisualLedgerView {
    fn set(&mut self, new_tree: LedgerViewNode) {
        self.tree = new_tree;
    }
}

pub struct RecordLedgerView {
    tree: LedgerViewNode,
}

impl fmt::Display for RecordLedgerView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", gen_output(&self.tree, &Colors::disabled(), 0))
    }
}

impl RecordLedgerView {
    pub fn new() -> Self {
        RecordLedgerView { tree: LedgerViewNode::default() }
    }
}

impl LedgerView for RecordLedgerView {
    fn set(&mut self, new_tree: LedgerViewNode) {
        self.tree = new_tree;
    }
}
