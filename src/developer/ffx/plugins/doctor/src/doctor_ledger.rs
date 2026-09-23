// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ledger_view::*;
use anyhow::{Result, anyhow};
use serde::Serialize;
use std::io::Write;

const DEFAULT_OUTCOME_VALUE: LedgerOutcome = LedgerOutcome::Success;
const DEFAULT_LEDGER_MODE: LedgerMode = LedgerMode::Normal;

// Main interface for LedgerNode.
pub trait LedgerNodeOp {
    fn add(&mut self, node: LedgerNode) -> usize;
    fn close(&mut self, depth: usize);
    fn set_outcome(&mut self, depth: usize, outcome: LedgerOutcome);
    fn make_view(&self, depth: usize, display_mode: LedgerViewMode) -> Option<LedgerViewNode>;
    fn make_all(&self, display_mode: LedgerViewMode) -> Vec<LedgerViewNode>;
}

#[derive(Serialize, Clone)]
pub struct LedgerNodeValue {
    data: String,
    outcome: LedgerOutcome,
    mode: LedgerMode,
}

#[derive(Serialize, Clone)]
pub struct LedgerNode {
    value: LedgerNodeValue,
    children: Vec<LedgerNode>,
    #[serde(skip)]
    is_closed: bool,
}

// Constructor and internal helper methods for LedgerNode.
impl LedgerNode {
    pub fn new(data: String, mode: LedgerMode) -> Self {
        LedgerNode {
            value: LedgerNodeValue { data, outcome: LedgerOutcome::Automatic, mode },
            children: Vec::new(),
            is_closed: false,
        }
    }

    // Return the outcome of the node. If the outcome is not set, infer it from its children's
    // output, the default value, and the fold function.
    fn get_node_outcome(&self) -> LedgerOutcome {
        if self.value.outcome != LedgerOutcome::Automatic {
            return self.value.outcome;
        }

        if self.children.len() == 0 {
            return DEFAULT_OUTCOME_VALUE;
        }

        self.children
            .iter()
            .fold(LedgerOutcome::ValidRangeStart, |acc, child| {
                acc.max(child.get_node_outcome().valid_or(LedgerOutcome::ValidRangeStart))
            })
            .valid_or(DEFAULT_OUTCOME_VALUE)
    }

    // Return the mode of the node. If the mode is not set, infer it from its children's mode.
    fn get_node_mode(&self) -> LedgerMode {
        if self.value.mode != LedgerMode::Automatic {
            return self.value.mode;
        }

        if self.children.len() == 0 {
            return DEFAULT_LEDGER_MODE;
        }

        return self
            .children
            .iter()
            .fold(LedgerMode::Verbose, |acc, child| acc.min(child.get_node_mode()));
    }

    // Determine based on settings if the node should be displayed.
    fn should_display_node(&self, display_mode: LedgerViewMode) -> bool {
        match display_mode {
            LedgerViewMode::Verbose => true,
            LedgerViewMode::Normal => match self.get_node_mode() {
                LedgerMode::Normal => true,
                _ => false,
            },
        }
    }

    // Recursive function to set the latest node's outcome at the specified end_depth.
    fn set_outcome_at_depth(
        &mut self,
        cur_depth: usize,
        end_depth: usize,
        outcome: LedgerOutcome,
    ) -> Result<()> {
        if cur_depth >= end_depth {
            self.value.outcome = outcome;
            return Ok(());
        } else if let Some(last_child) = self.children.last_mut() {
            return last_child.set_outcome_at_depth(cur_depth + 1, end_depth, outcome);
        } else {
            return Err(anyhow!("Cannot set outcome at depth {}, node does not exist", end_depth));
        }
    }

    // Add new node as a child of the most current open node, return depth.
    fn add_at_max_depth(&mut self, node: LedgerNode, depth: usize) -> Result<usize> {
        if self.is_closed {
            return Err(anyhow!("Add error: Ledger node at depth {} is closed", depth));
        }

        if let Some(last_child) = self.children.last_mut() {
            if last_child.is_closed == false {
                return last_child.add_at_max_depth(node, depth + 1);
            }
        }

        self.children.push(node);
        return Ok(depth + 1);
    }

    // Recursive function to close the latest node at the specified end_depth.
    fn close_at_depth(&mut self, cur_depth: usize, end_depth: usize) -> Result<()> {
        if cur_depth >= end_depth {
            self.is_closed = true;
            return Ok(());
        } else if let Some(last_child) = self.children.last_mut() {
            return last_child.close_at_depth(cur_depth + 1, end_depth);
        } else {
            return Err(anyhow!("Cannot close node at depth {}, node does not exist", end_depth));
        }
    }

    // Recursive function to calculate the latest node's outcome at the specified end_depth.
    fn calc_outcome_at_depth(&self, depth: usize) -> LedgerOutcome {
        if depth == 0 {
            return self.get_node_outcome();
        } else if let Some(last_child) = self.children.last() {
            return last_child.calc_outcome_at_depth(depth - 1);
        } else {
            return LedgerOutcome::Automatic;
        }
    }

    // Make a simplified (display) node tree.
    fn make_node_view(&self, display_mode: LedgerViewMode) -> Option<LedgerViewNode> {
        if !self.should_display_node(display_mode) {
            return None;
        }

        let outcome = LedgerViewOutcome::from(self.get_node_outcome());
        let mut output_node =
            LedgerViewNode { data: self.value.data.clone(), outcome, children: Vec::new() };

        for child in self.children.iter() {
            if let Some(node) = child.make_node_view(display_mode) {
                output_node.children.push(node);
            }
        }

        return Some(output_node);
    }
}

impl LedgerNodeOp for LedgerNode {
    // Add node as a child of the most recent open node, return depth
    fn add(&mut self, node: LedgerNode) -> usize {
        return self.add_at_max_depth(node, 0).expect("Failed to add ledger node");
    }

    // Close the most recent node at the specified depth.
    fn close(&mut self, depth: usize) {
        self.close_at_depth(0, depth).expect("Failed to close ledger node");
    }

    // Set the most recent node's outcome at the specified depth.
    fn set_outcome(&mut self, depth: usize, outcome: LedgerOutcome) {
        self.set_outcome_at_depth(0, depth, outcome).expect("Failed to set ledger node outcome");
    }

    // Return the display node tree for the most recent node at the specified depth.
    fn make_view(&self, depth: usize, display_mode: LedgerViewMode) -> Option<LedgerViewNode> {
        if depth == 0 {
            return self.make_node_view(display_mode);
        } else {
            return match self.children.last() {
                Some(child) => child.make_view(depth - 1, display_mode),
                None => None,
            };
        }
    }

    // Return all display nodes at depth 1.
    fn make_all(&self, display_mode: LedgerViewMode) -> Vec<LedgerViewNode> {
        let mut output = Vec::<LedgerViewNode>::new();
        for child in &self.children {
            if let Some(node) = child.make_node_view(display_mode) {
                output.push(node);
            }
        }
        output
    }
}

#[derive(Serialize, Copy, Clone, PartialOrd, Ord, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LedgerOutcome {
    ValidRangeStart,
    Success,
    Warning,
    Failure,
    ValidRangeEnd,
    Automatic,
    // Values below are ignored when computing the outcome of parent.
    SoftWarning,
    Info,
}

impl LedgerOutcome {
    pub fn valid_or(self, default: LedgerOutcome) -> Self {
        if self > LedgerOutcome::ValidRangeStart && self < LedgerOutcome::ValidRangeEnd {
            return self;
        } else {
            return default;
        }
    }
}

// Mode type for nodes.
#[derive(Serialize, Debug, Copy, Clone, PartialOrd, Ord, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LedgerMode {
    Normal,
    Verbose,
    Automatic,
}

#[derive(Copy, Clone)]
pub enum LedgerViewMode {
    Normal,
    Verbose,
}

// DoctorLedger:
// Builds and keeps track of a LedgerNode tree to automatically decide when and what display data
// to send (to a member variable that implements LedgerView). The display data consists of a
// LedgerViewNode tree that is constructed from a LedgerNode tree and the ledger mode settings.
//
// Internal representation of the LedgerNode tree:
// * Root: No data.
// * First level nodes: Data that can be modified. Invoke display when this node is closed.
// * 2nd Level+: Data that can be modified.
//
pub struct LedgerNodeGuard<'a, W: Write> {
    ledger: &'a mut DoctorLedger<W>,
    depth: usize,
    is_closed: bool,
}

impl<'a, W: Write> LedgerNodeGuard<'a, W> {
    pub fn new(ledger: &'a mut DoctorLedger<W>, depth: usize) -> Self {
        Self { ledger, depth, is_closed: false }
    }

    pub fn add_node(&mut self, data: &str, mode: LedgerMode) -> LedgerNodeGuard<'_, W> {
        let depth = self.ledger.add_node_internal(data, mode);
        LedgerNodeGuard::new(self.ledger, depth)
    }

    pub fn add(&mut self, node: LedgerNode) -> LedgerNodeGuard<'_, W> {
        let depth = self.ledger.add_internal(node);
        LedgerNodeGuard::new(self.ledger, depth)
    }

    pub fn set_outcome(mut self, outcome: LedgerOutcome) {
        self.is_closed = true;
        self.ledger.set_outcome_internal(self.depth, outcome);
    }

    pub fn close(mut self) {
        if !self.is_closed {
            self.is_closed = true;
            self.ledger.close_internal(self.depth);
        }
    }

    pub fn add_node_with_outcome(&mut self, data: &str, mode: LedgerMode, outcome: LedgerOutcome) {
        let node = self.add_node(data, mode);
        node.set_outcome(outcome);
    }

    pub fn add_with_outcome(&mut self, node: LedgerNode, outcome: LedgerOutcome) {
        let guard = self.add(node);
        guard.set_outcome(outcome);
    }

    pub fn calc_outcome_at_next_depth(&self) -> LedgerOutcome {
        self.ledger.calc_outcome(self.depth + 1)
    }

    pub fn calc_outcome(&self) -> LedgerOutcome {
        self.ledger.calc_outcome(self.depth)
    }

    pub fn get_ledger_mode(&self) -> LedgerViewMode {
        self.ledger.get_ledger_mode()
    }

    pub fn write_all(&self, ledger_view: &mut dyn LedgerView) -> String {
        self.ledger.write_all(ledger_view)
    }
}

impl<'a, W: Write> Drop for LedgerNodeGuard<'a, W> {
    fn drop(&mut self) {
        if !self.is_closed {
            self.ledger.close_internal(self.depth);
        }
    }
}

pub struct DoctorLedger<W: Write> {
    pub writer: W,
    root_node: LedgerNode,
    ledger_view: Box<dyn LedgerView>,
    ledger_mode: LedgerViewMode,
}

impl<W: Write> DoctorLedger<W> {
    pub fn new(writer: W, view: Box<dyn LedgerView>, mode: LedgerViewMode) -> DoctorLedger<W> {
        DoctorLedger {
            writer,
            root_node: LedgerNode::new("".to_string(), LedgerMode::Normal),
            ledger_view: view,
            ledger_mode: mode,
        }
    }

    pub fn root_node(&self) -> &LedgerNode {
        &self.root_node
    }

    pub fn into_root_node(self) -> LedgerNode {
        self.root_node
    }

    fn add_internal(&mut self, node: LedgerNode) -> usize {
        return self.root_node.add(node);
    }

    fn add_node_internal(&mut self, data: &str, mode: LedgerMode) -> usize {
        return self.add_internal(LedgerNode::new(data.to_string(), mode));
    }

    pub fn calc_outcome(&self, depth: usize) -> LedgerOutcome {
        return self.root_node.calc_outcome_at_depth(depth);
    }

    fn set_outcome_internal(&mut self, depth: usize, outcome: LedgerOutcome) {
        self.root_node.set_outcome(depth, outcome);
        self.close_internal(depth);
    }

    fn close_internal(&mut self, depth: usize) {
        if depth == 1 {
            self.display();
        }
        self.root_node.close(depth);
    }

    pub fn add(&mut self, node: LedgerNode) -> LedgerNodeGuard<'_, W> {
        let depth = self.add_internal(node);
        LedgerNodeGuard::new(self, depth)
    }

    pub fn add_node(&mut self, data: &str, mode: LedgerMode) -> LedgerNodeGuard<'_, W> {
        let depth = self.add_node_internal(data, mode);
        LedgerNodeGuard::new(self, depth)
    }

    pub fn root_guard(&mut self) -> LedgerNodeGuard<'_, W> {
        LedgerNodeGuard::new(self, 0)
    }

    pub fn get_ledger_mode(&self) -> LedgerViewMode {
        return self.ledger_mode;
    }

    fn display(&mut self) {
        match self.root_node.make_view(1, self.ledger_mode) {
            Some(node) => {
                self.ledger_view.set(node);
                write!(self.writer, "{}", self.ledger_view)
                    .expect("Failed to write to ledger writer");
            }
            None => (),
        }
    }

    pub fn write_all(&self, ledger_view: &mut dyn LedgerView) -> String {
        let mut output = "".to_string();
        for node in self.root_node.make_all(self.ledger_mode) {
            ledger_view.set(node);
            output = format!("{}{}", output, ledger_view);
        }
        return output;
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use ffx_doctor_test_utils::MockWriter;
    use std::fmt;
    use termio::Colors;

    const MODE_VERBOSE: LedgerMode = LedgerMode::Verbose;
    const MODE_NORMAL: LedgerMode = LedgerMode::Normal;
    const MODE_DEFAULT: LedgerMode = LedgerMode::Automatic;
    const INDENT_STR: &str = "    ";

    pub fn doctorledger_test_new(
        view: Box<dyn LedgerView>,
        mode: LedgerViewMode,
    ) -> DoctorLedger<MockWriter> {
        DoctorLedger::<MockWriter>::new(MockWriter::new(), view, mode)
    }

    pub fn doctorledger_debug(ledger: &DoctorLedger<MockWriter>) -> String {
        ledger.writer.get_data()
    }

    struct FakeLedgerView {
        tree: LedgerViewNode,
    }

    impl FakeLedgerView {
        pub fn new() -> Self {
            FakeLedgerView { tree: LedgerViewNode::default() }
        }
        fn gen_output(&self, parent_node: &LedgerViewNode, indent_level: usize) -> String {
            let mut output_str = format!(
                "{}[{}] {}\n",
                INDENT_STR.repeat(indent_level),
                parent_node.outcome.format(&Colors::disabled()),
                &parent_node.data
            );

            for child_node in &parent_node.children {
                let child_str = self.gen_output(child_node, indent_level + 1);
                output_str = format!("{}{}", output_str, child_str);
            }

            return output_str;
        }
    }

    impl fmt::Display for FakeLedgerView {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.gen_output(&self.tree, 0))
        }
    }

    impl LedgerView for FakeLedgerView {
        fn set(&mut self, new_tree: LedgerViewNode) {
            self.tree = new_tree;
        }
    }

    #[fuchsia::test]
    async fn test_outcome() {
        let ledger_view = Box::new(FakeLedgerView::new());
        let mut ledger = doctorledger_test_new(ledger_view, LedgerViewMode::Verbose);

        ledger
            .add(LedgerNode::new("a".to_string(), MODE_VERBOSE))
            .set_outcome(LedgerOutcome::Success);
        ledger
            .add(LedgerNode::new("b".to_string(), MODE_VERBOSE))
            .set_outcome(LedgerOutcome::Warning);
        ledger
            .add(LedgerNode::new("c".to_string(), MODE_VERBOSE))
            .set_outcome(LedgerOutcome::Failure);
        ledger.add(LedgerNode::new("d".to_string(), MODE_VERBOSE)).set_outcome(LedgerOutcome::Info);

        assert_eq!(
            doctorledger_debug(&ledger),
            "\
                \n[✓] a\
                \n[!] b\
                \n[✗] c\
                \n[i] d\n"
        );
    }

    fn setup_simple_mode(ledger: &mut DoctorLedger<MockWriter>) {
        ledger
            .add(LedgerNode::new("a".to_string(), MODE_VERBOSE))
            .set_outcome(LedgerOutcome::Success);
        ledger
            .add(LedgerNode::new("b".to_string(), MODE_NORMAL))
            .set_outcome(LedgerOutcome::Warning);
        ledger
            .add(LedgerNode::new("c".to_string(), MODE_DEFAULT))
            .set_outcome(LedgerOutcome::Failure);
        ledger
            .add(LedgerNode::new("d".to_string(), MODE_VERBOSE))
            .set_outcome(LedgerOutcome::Failure);
        ledger
            .add(LedgerNode::new("e".to_string(), MODE_NORMAL))
            .set_outcome(LedgerOutcome::Success);
        ledger.add(LedgerNode::new("f".to_string(), MODE_NORMAL)).set_outcome(LedgerOutcome::Info);
    }

    #[fuchsia::test]
    async fn test_mode_verbose() {
        let ledger_view = Box::new(FakeLedgerView::new());
        let mut ledger = doctorledger_test_new(ledger_view, LedgerViewMode::Verbose);
        setup_simple_mode(&mut ledger);

        assert_eq!(
            doctorledger_debug(&ledger),
            "\
                \n[✓] a\
                \n[!] b\
                \n[✗] c\
                \n[✗] d\
                \n[✓] e\
                \n[i] f\
                \n"
        );
    }

    #[fuchsia::test]
    async fn test_mode_normal() {
        let ledger_view = Box::new(FakeLedgerView::new());
        let mut ledger = doctorledger_test_new(ledger_view, LedgerViewMode::Normal);
        setup_simple_mode(&mut ledger);

        assert_eq!(
            doctorledger_debug(&ledger),
            "\
                \n[!] b\
                \n[✗] c\
                \n[✓] e\
                \n[i] f\n"
        );
    }

    #[fuchsia::test]
    async fn test_group_outcome() {
        let ledger_view = Box::new(FakeLedgerView::new());
        let mut ledger = doctorledger_test_new(ledger_view, LedgerViewMode::Normal);

        {
            let mut main_node = ledger.add(LedgerNode::new("a".to_string(), MODE_NORMAL));
            main_node
                .add(LedgerNode::new("1".to_string(), MODE_VERBOSE))
                .set_outcome(LedgerOutcome::Success);
            main_node
                .add(LedgerNode::new("2".to_string(), MODE_VERBOSE))
                .set_outcome(LedgerOutcome::Success);
            main_node
                .add(LedgerNode::new("3".to_string(), MODE_VERBOSE))
                .set_outcome(LedgerOutcome::Success);
        }
        {
            let mut main_node = ledger.add(LedgerNode::new("b".to_string(), MODE_NORMAL));
            main_node
                .add(LedgerNode::new("1".to_string(), MODE_VERBOSE))
                .set_outcome(LedgerOutcome::Success);
            main_node
                .add(LedgerNode::new("2".to_string(), MODE_VERBOSE))
                .set_outcome(LedgerOutcome::Warning);
            main_node
                .add(LedgerNode::new("3".to_string(), MODE_VERBOSE))
                .set_outcome(LedgerOutcome::Success);
        }
        {
            let mut main_node = ledger.add(LedgerNode::new("c".to_string(), MODE_NORMAL));
            main_node
                .add(LedgerNode::new("1".to_string(), MODE_VERBOSE))
                .set_outcome(LedgerOutcome::Success);
            main_node
                .add(LedgerNode::new("2".to_string(), MODE_VERBOSE))
                .set_outcome(LedgerOutcome::SoftWarning);
            main_node
                .add(LedgerNode::new("3".to_string(), MODE_VERBOSE))
                .set_outcome(LedgerOutcome::Success);
        }
        {
            let mut main_node = ledger.add(LedgerNode::new("d".to_string(), MODE_NORMAL));
            main_node
                .add(LedgerNode::new("1".to_string(), MODE_VERBOSE))
                .set_outcome(LedgerOutcome::Success);
            main_node
                .add(LedgerNode::new("2".to_string(), MODE_VERBOSE))
                .set_outcome(LedgerOutcome::Failure);
            main_node
                .add(LedgerNode::new("3".to_string(), MODE_VERBOSE))
                .set_outcome(LedgerOutcome::Success);
        }

        assert_eq!(
            doctorledger_debug(&ledger),
            "\
                \n[✓] a\
                \n[!] b\
                \n[✓] c\
                \n[✗] d\
                \n"
        );
    }
}
