// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_codec_fdomain::Value as FidlValue;
use futures::FutureExt;
use futures::channel::oneshot::channel as oneshot_channel;
use futures::future::{BoxFuture, ready};
use num::bigint::BigInt;
use num::rational::BigRational;
use num::{CheckedDiv, FromPrimitive};
use std::collections::HashMap;
use std::fmt::Write;
use std::iter::repeat_with;
use std::sync::{Arc, Mutex};

use crate::error::Result;
use crate::frame::{CaptureMapEntry, CaptureSet, Frame};
use crate::interpreter::{Exception, Interpreter, InterpreterInner};
use crate::parser::{Mutability, Node, ParameterList, Span, StringElement};
use crate::value::{
    Invocable, PlaygroundValue, RangeCursor, ReplayableIterator, Value, ValueExt,
    playground_semantic_compare,
};

impl Exception {
    /// Convert this [`Exception`] into the crate level `Error` type. This is
    /// the same as the general `into` but its simpler signature can give Rust's
    /// type inference a boost.
    pub(crate) fn into_err(self) -> crate::error::Error {
        self.into()
    }
}

/// Convert two values to numbers. If the conversion succeeds, use the given
/// callback to do math on the numbers and return the result.
fn try_numeric_math(
    a: Value,
    b: Value,
    f: impl FnOnce(BigRational, BigRational) -> Result<Value>,
) -> Option<Result<Value>> {
    let a: Option<BigRational> = a.try_big_num().ok();
    let b: Option<BigRational> = b.try_big_num().ok();

    if let (Some(a), Some(b)) = (a, b) { Some(f(a, b)) } else { None }
}

/// Parse an integer.
fn parse_integer(string: &str) -> BigInt {
    let string = string.replace("_", "");

    let (data, radix) = string.strip_prefix("0x").map(|x| (x, 16)).unwrap_or((string.as_str(), 10));
    BigInt::parse_bytes(data.as_bytes(), radix).unwrap()
}

/// Used to walk through the parse tree and create a closure which can be used
/// to run the given code.
pub struct Visitor {
    next_slot: usize,
    next_scope_id: usize,
    scope_id: usize,
    scope_stack: Vec<usize>,
    allocated_slots: HashMap<(String, usize), usize>,
    captured_slots: HashMap<String, usize>,
    constant_slots: Vec<usize>,
    global_fs_root: Option<usize>,
    global_pwd: Option<usize>,
}

impl Visitor {
    /// Constructs a new visitor.
    pub fn new(global_fs_root: Option<usize>, global_pwd: Option<usize>) -> Self {
        Visitor {
            next_slot: 0,
            next_scope_id: 1,
            scope_id: 0,
            scope_stack: Vec::new(),
            allocated_slots: HashMap::new(),
            captured_slots: HashMap::new(),
            constant_slots: Vec::new(),
            global_fs_root,
            global_pwd,
        }
    }

    /// Get the names of all variables that were declared in the top level scope.
    pub fn get_top_level_variable_decls(&self) -> impl Iterator<Item = (&String, Mutability)> {
        self.allocated_slots.iter().filter_map(|((name, scope), slot)| {
            if *scope == 0 {
                Some((
                    name,
                    if self.constant_slots.contains(slot) {
                        Mutability::Constant
                    } else {
                        Mutability::Mutable
                    },
                ))
            } else {
                None
            }
        })
    }

    /// Consume this object and return two hashmaps. The first maps capturing
    /// variable names to the slots they were assigned, the second does the same
    /// for top-level declared variables.
    pub fn into_slot_data(self) -> (HashMap<String, usize>, HashMap<String, usize>) {
        (
            self.captured_slots,
            self.allocated_slots
                .into_iter()
                .filter_map(|((name, scope), id)| if scope == 0 { Some((name, id)) } else { None })
                .collect(),
        )
    }

    /// How many slots are needed for the frame used to run the result of this visitor.
    pub fn slots_needed(&self) -> usize {
        self.next_slot
    }

    /// Enter a new scope. Variables declared by statements visited after this
    /// point will shadow variables of the same name from previously, until
    /// [exit_scope] is called.
    fn enter_scope(&mut self) {
        self.scope_stack.push(self.scope_id);
        self.scope_id = self.next_scope_id;
        self.next_scope_id += 1;
    }

    /// Exit a scope. This undoes the effect of [enter_scope]
    fn exit_scope(&mut self) {
        self.scope_id = self.scope_stack.pop().expect("Tried to exit bottom scope!");
    }

    /// Allocate a new slot in the current frame for the given variable name, in
    /// the present scope. This effectively allocates the "storage" behind the
    /// declaration of a new variable. The `name` argument is the name of the
    /// variable.
    fn allocate_slot(&mut self, name: &str, mutability: Mutability) -> usize {
        let name = name.to_owned();
        let ident = (name, self.scope_id);
        let slot = if let Some(slot) = self.allocated_slots.get(&ident) {
            *slot
        } else {
            let slot = self.next_slot;
            self.next_slot += 1;
            let _ = self.allocated_slots.insert(ident, slot);
            slot
        };

        if mutability.is_constant() {
            if !self.constant_slots.contains(&slot) {
                self.constant_slots.push(slot);
            }
        } else {
            self.constant_slots.retain(|x| *x != slot)
        }

        slot
    }

    /// Get the ID of the slot corresponding to a particular variable name in
    /// the current frame and scope, and whether that slot is known to be const.
    ///
    /// If the variable hasn't been declared, return `None`.
    fn fetch_slot_no_capture(&self, name: &str) -> Option<(usize, Mutability)> {
        let name = name.to_owned();
        let mut ident = (name, 0);

        for scope_id in [self.scope_id].iter().chain(self.scope_stack.iter().rev()).copied() {
            ident.1 = scope_id;
            if let Some(id) = self.allocated_slots.get(&ident) {
                let mutability = if self.constant_slots.contains(id) {
                    Mutability::Constant
                } else {
                    Mutability::Mutable
                };
                return Some((*id, mutability));
            }
        }

        None
    }

    /// Get the ID of the slot corresponding to a particular variable name in
    /// the current frame and scope, and whether that slot is known to be const.
    ///
    /// If the variable hasn't been declared, allocate a new slot as a capture.
    fn fetch_slot(&mut self, name: &str) -> (usize, Mutability) {
        if let Some(capture) = self.fetch_slot_no_capture(name) {
            return capture;
        }

        let slot = *self.captured_slots.entry(name.to_owned()).or_insert_with(|| {
            let slot = self.next_slot;
            self.next_slot += 1;
            slot
        });

        if name == "fs_root" && self.global_fs_root.is_none() {
            self.global_fs_root = Some(slot)
        }

        if name == "pwd" && self.global_pwd.is_none() {
            self.global_pwd = Some(slot)
        }

        (slot, Mutability::Mutable)
    }

    /// Given another visitor which compiled some sub-scope beneath this one,
    /// return a list of [`CaptureMapEntry`]s indicating what slots in the frame
    /// running this visitor's unit should captured by what slots in the frame
    /// of the child visitor's unit.
    fn capture_map_for(&mut self, other: &Visitor) -> Vec<CaptureMapEntry> {
        other
            .captured_slots
            .iter()
            .map(|(name, &slot_to)| {
                let (slot_from, mutability) = self.fetch_slot(name);
                CaptureMapEntry { slot_from, slot_to, mutability }
            })
            .collect()
    }

    pub fn visit<'a>(
        &mut self,
        node: Node<'a>,
    ) -> Arc<
        dyn (for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>>)
            + Send
            + Sync,
    > {
        match node {
            Node::Add(x, y) => Arc::new(self.visit_add(*x, *y)),
            Node::Assignment(x, y) => Arc::new(self.visit_assignment(*x, *y)),
            Node::Async(x) => Arc::new(self.visit_async(*x)),
            Node::BareString(s) => Arc::new(self.visit_bare_string(*s.fragment())),
            Node::Block(v) => Arc::new(self.visit_block(v)),
            Node::Divide(x, y) => Arc::new(self.visit_divide(*x, *y)),
            Node::EQ(a, b) => Arc::new(self.visit_eq(*a, *b)),
            Node::FunctionDecl { identifier, parameters, body } => {
                Arc::new(self.visit_function_decl(*identifier.fragment(), parameters, *body))
            }
            Node::GE(a, b) => Arc::new(self.visit_ge(*a, *b)),
            Node::GT(a, b) => Arc::new(self.visit_gt(*a, *b)),
            Node::Identifier(s) => Arc::new(self.visit_identifier(*s.fragment())),
            Node::If { condition, body, else_ } => {
                Arc::new(self.visit_if(*condition, *body, else_.map(|x| *x)))
            }
            Node::Import(a, b) => {
                let b = b.expect("Top level visitor should not handle import statements!");
                Arc::new(self.visit_import(*a.fragment(), b.fragment()))
            }
            Node::Integer(s) => Arc::new(self.visit_integer(*s.fragment())),
            Node::Invocation(n, a) => Arc::new(self.visit_invocation(*n.fragment(), a)),
            Node::Iterate(a, b) => Arc::new(self.visit_iterate(*a, *b)),
            Node::LE(a, b) => Arc::new(self.visit_le(*a, *b)),
            Node::LT(a, b) => Arc::new(self.visit_lt(*a, *b)),
            Node::Label(s) => Arc::new(self.visit_label(*s.fragment())),
            Node::Lambda { parameters, body } => {
                Arc::new(self.visit_lambda("λ", parameters, *body))
            }
            Node::List(v) => Arc::new(self.visit_list(v)),
            Node::LogicalAnd(a, b) => Arc::new(self.visit_logical_and_or(*a, *b, "&&", false)),
            Node::LogicalNot(a) => Arc::new(self.visit_logical_not(*a)),
            Node::LogicalOr(a, b) => Arc::new(self.visit_logical_and_or(*a, *b, "||", true)),
            Node::Lookup(n, s) => Arc::new(self.visit_lookup(*n, *s)),
            Node::Multiply(x, y) => Arc::new(self.visit_multiply(*x, *y)),
            Node::NE(a, b) => Arc::new(self.visit_ne(*a, *b)),
            Node::Negate(v) => Arc::new(self.visit_negate(*v)),
            Node::Object(l, v) => Arc::new(self.visit_object(l, v)),
            Node::Pipe(a, b) => Arc::new(self.visit_pipe(*a, *b)),
            Node::Program(v) => Arc::new(self.visit_program(v)),
            Node::Range(a, b, i) => Arc::new(self.visit_range(*a, *b, i)),
            Node::Real(a) => Arc::new(self.visit_real(*a.fragment())),
            Node::String(s) => Arc::new(self.visit_string(s)),
            Node::Subtract(x, y) => Arc::new(self.visit_subtract(*x, *y)),
            Node::VariableDecl { identifier, value, mutability } => {
                Arc::new(self.visit_variable_decl(*identifier.fragment(), *value, mutability))
            }
            Node::True => Arc::new(|_, _| async move { Ok(Value::Bool(true)) }.boxed()),
            Node::False => Arc::new(|_, _| async move { Ok(Value::Bool(false)) }.boxed()),
            Node::Null => Arc::new(|_, _| async move { Ok(Value::Null) }.boxed()),

            // We should never try to execute a parse tree with these in it.
            Node::Error => panic!("Invalid output from parser"),
        }
    }

    fn visit_lvalue<'a>(
        &mut self,
        node: Node<'a>,
    ) -> Arc<
        dyn (for<'f> Fn(
                BoxFuture<'static, Result<Value>>,
                &Arc<InterpreterInner>,
                &'f Mutex<Frame>,
            ) -> BoxFuture<'f, Result<Value>>)
            + Send
            + Sync,
    > {
        match node {
            Node::Identifier(identifier) => {
                let lookup = Arc::new(self.visit_identifier(*identifier.fragment()));
                let (slot, mutability) = self.fetch_slot(*identifier.fragment());
                let identifier = (*identifier.fragment()).to_owned();

                if mutability.is_constant() {
                    Arc::new(move |_, _, _| {
                        let identifier = identifier.clone();
                        async move { Err(Exception::AssignToConst(identifier).into()) }.boxed()
                    })
                } else {
                    Arc::new(move |value, inner, frame| {
                        let task = frame.lock().unwrap().assign_future_if_not_const(slot, value);

                        if let Some(task) = task {
                            inner.push_task(task);
                            lookup(&inner, frame).boxed()
                        } else {
                            let identifier = identifier.clone();
                            async move { Err(Exception::AssignToConst(identifier).into()) }.boxed()
                        }
                    })
                }
            }
            Node::Lookup(receiver, key) => {
                let key = self.visit(*key);
                let receiver_set = Arc::new(self.visit_lvalue((*receiver).clone()));
                let receiver = self.visit(*receiver);

                Arc::new(move |value, inner, frame| {
                    let receiver = Arc::clone(&receiver);
                    let receiver_set = Arc::clone(&receiver_set);
                    let inner = Arc::clone(inner);
                    let key = Arc::clone(&key);

                    async move {
                        let key = key(&inner, frame).await?;
                        let new_value = {
                            let new_value = receiver(&inner, frame).await?;
                            match new_value {
                                Value::Object(mut h) => {
                                    let Value::String(key) = key else {
                                        return Err(Exception::NonStringObjectKey.into());
                                    };

                                    let mut value = Some(value.await?);
                                    for (name, existing_value) in h.iter_mut() {
                                        if name == &key {
                                            *existing_value = value.take().unwrap();
                                            break;
                                        }
                                    }

                                    if let Some(value) = value {
                                        h.push((key, value));
                                    }

                                    Value::Object(h)
                                }
                                Value::List(mut l) => {
                                    let key: usize = key.try_usize().map_err(|_| {
                                        Exception::NonPositiveIntegerListKey.into_err()
                                    })?;

                                    if key < l.len() {
                                        l[key] = value.await?;
                                    } else {
                                        Err(Exception::ListIndexOutOfRange.into_err())?
                                    }

                                    Value::List(l)
                                }
                                _ => Err(Exception::LookupNotSupported.into_err())?,
                            }
                        };
                        receiver_set(ready(Ok(new_value)).boxed(), &inner, frame).await
                    }
                    .boxed()
                })
            }

            // We should never try to execute a parse tree with these in it.
            Node::Error => panic!("Invalid output from parser"),
            _ => Arc::new(|_, _, _| async { Err(Exception::BadLValue.into()) }.boxed()),
        }
    }

    fn visit_add<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        self.binop(a, b, |a, b| match (a, b) {
            (Value::List(mut a), Value::List(mut b)) => {
                a.append(&mut b);
                Ok(Value::List(a))
            }
            (Value::String(mut a), Value::String(b)) => {
                a.push_str(&b);
                Ok(FidlValue::String(a))
            }
            (a, b) => {
                try_numeric_math(a, b, |a, b| Ok(Value::OutOfLine(PlaygroundValue::Num(a + b))))
                    .unwrap_or_else(|| Err(Exception::BadAdditionOperands.into()))
            }
        })
    }

    fn visit_assignment<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let b = self.visit(b);
        let a = self.visit_lvalue(a);

        move |inner, frame| {
            let a = Arc::clone(&a);
            let b = Arc::clone(&b);
            let inner = Arc::clone(&inner);

            async move {
                let b = b(&inner, frame).await?;
                a(ready(Ok(b)).boxed(), &inner, frame).await
            }
            .boxed()
        }
    }

    fn visit_async<'a>(
        &mut self,
        child: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        enum Ret<A, B> {
            A(A),
            B(B),
        }

        let ret = if let Node::VariableDecl { identifier, value, mutability } = child {
            Ret::A(self.visit_variable_decl_async(*identifier.fragment(), *value, mutability))
        } else {
            let mut visitor = Visitor::new(self.global_fs_root.clone(), self.global_pwd.clone());
            let task = visitor.visit(child);
            let capture_map = self.capture_map_for(&visitor);
            let slots_needed = visitor.slots_needed();

            Ret::B(move |inner: &Arc<InterpreterInner>, frame: &Mutex<Frame>| {
                let capture_set = {
                    let mut frame = frame.lock().unwrap();
                    CaptureSet::new(&mut frame, &capture_map)
                };

                let mut body_frame = Frame::new(slots_needed);
                body_frame.apply_capture(&capture_set);
                let body_frame = Mutex::new(body_frame);
                let inner_clone = Arc::clone(&inner);
                let task = Arc::clone(&task);
                inner.push_task(async move {
                    let _ = task(&inner_clone, &body_frame).await;
                });
            })
        };

        move |inner, frame| {
            match &ret {
                Ret::A(x) => x(inner, frame),
                Ret::B(x) => x(inner, frame),
            }
            async { Ok(Value::Null) }.boxed()
        }
    }

    fn visit_bare_string(
        &mut self,
        string: &str,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let string = string.to_owned();
        move |_, _| {
            let string = string.clone();
            async move { Ok(Value::String(string)) }.boxed()
        }
    }

    fn visit_block<'a>(
        &mut self,
        nodes: Vec<Node<'a>>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        self.enter_scope();
        let body = self.visit_program(nodes);
        self.exit_scope();

        body
    }

    fn visit_divide<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        self.binop(a, b, |a, b| {
            try_numeric_math(a, b, |a, b| {
                a.checked_div(&b)
                    .ok_or_else(|| Exception::DivisionByZero.into())
                    .map(|x| Value::OutOfLine(PlaygroundValue::Num(x)))
            })
            .unwrap_or_else(|| Err(Exception::BadNumericOperands("//").into()))
        })
    }

    fn visit_eq<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        self.binop(a, b, |a, b| {
            Ok(Value::Bool(playground_semantic_compare(&a, &b).map(|x| x.is_eq()).unwrap_or(false)))
        })
    }

    fn visit_ge<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        self.binop(a, b, |a, b| {
            Ok(Value::Bool(playground_semantic_compare(&a, &b).map(|x| x.is_ge()).unwrap_or(false)))
        })
    }

    fn visit_gt<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        self.binop(a, b, |a, b| {
            Ok(Value::Bool(playground_semantic_compare(&a, &b).map(|x| x.is_gt()).unwrap_or(false)))
        })
    }

    fn visit_identifier(
        &mut self,
        string: &str,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let id = self.fetch_slot(string).0;

        move |_, frame| {
            async move {
                let fut = frame.lock().unwrap().get(id);
                fut.await
            }
            .boxed()
        }
    }

    fn visit_if<'a>(
        &mut self,
        condition: Node<'a>,
        body: Node<'a>,
        else_: Option<Node<'a>>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let condition = self.visit(condition);
        let body = self.visit(body);
        let else_ = else_.map(|x| self.visit(x));

        move |inner, frame| {
            let condition = Arc::clone(&condition);
            let body = Arc::clone(&body);
            let else_ = else_.clone();
            let inner = Arc::clone(inner);
            async move {
                match condition(&inner, frame).await? {
                    Value::Bool(b) => {
                        if b {
                            body(&inner, frame).await
                        } else if let Some(else_) = else_.as_ref() {
                            else_(&inner, frame).await
                        } else {
                            Ok(Value::Null)
                        }
                    }
                    _ => Err(Exception::NonBooleanConditional.into()),
                }
            }
            .boxed()
        }
    }

    fn visit_import(
        &mut self,
        path: &str,
        name: &str,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let path = path.to_owned();
        let name = self.allocate_slot(name, Mutability::Constant);
        let fs_root = self.global_fs_root.clone().unwrap_or_else(|| self.fetch_slot("fs_root").0);
        let pwd = self.global_pwd.clone().unwrap_or_else(|| self.fetch_slot("pwd").0);

        move |inner, frame| {
            let path = path.clone();
            let inner = Arc::clone(inner);

            async move {
                let (fs_root, pwd) = {
                    let frame = frame.lock().unwrap();
                    (frame.get(fs_root), frame.get(pwd))
                };

                let fs_root = fs_root.await?;
                let pwd = pwd.await?;

                let fut = async move {
                    let interpreter =
                        Interpreter::new_with_inner(Arc::clone(&inner), fs_root, pwd).await;
                    let globals = interpreter.run_isolated_import(path).await?;
                    globals.to_object().await
                };
                let fut = frame.lock().unwrap().assign_future_ignore_const(name, fut);
                fut.await;
                Ok(Value::Null)
            }
            .boxed()
        }
    }

    fn visit_integer(
        &mut self,
        string: &str,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let value = parse_integer(string);

        move |_, _| {
            let value = value.clone();
            async move { Ok(Value::OutOfLine(PlaygroundValue::Num(BigRational::from_integer(value)))) }.boxed()
        }
    }

    fn visit_invocation<'a>(
        &mut self,
        target: &str,
        args: Vec<Node<'a>>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let target = Arc::new(self.visit_identifier(target));
        let args = Arc::new(args.into_iter().map(|x| self.visit(x)).collect::<Vec<_>>());
        let underscore_arg_id = self.fetch_slot_no_capture("_").map(|(x, _)| x);

        move |inner, frame| {
            let target = Arc::clone(&target);
            let args = Arc::clone(&args);
            let inner = Arc::clone(inner);

            async move {
                let invocable = target(&inner, frame).await?;
                let mut args_resolved = Vec::new();
                for arg in args.iter() {
                    let arg = arg(&inner, frame).await?;
                    args_resolved.push(arg);
                }
                let v = if let Some(underscore_arg_id) = underscore_arg_id {
                    let v = frame.lock().unwrap().get(underscore_arg_id);
                    Some(v.await?)
                } else {
                    None
                };
                inner.invoke_value(invocable, args_resolved, v).await
            }
            .boxed()
        }
    }

    fn visit_iterate<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let a = self.visit(a);
        self.enter_scope();
        let mut body_visitor = Visitor::new(self.global_fs_root.clone(), self.global_pwd.clone());
        let underscore_arg_id = body_visitor.allocate_slot("_", Mutability::Mutable);
        let b = body_visitor.visit(b);
        let capture_map = self.capture_map_for(&body_visitor);
        let slots_needed = body_visitor.slots_needed();
        self.exit_scope();

        move |inner, frame| {
            let a = Arc::clone(&a);
            let b = Arc::clone(&b);
            let inner = Arc::clone(inner);
            let capture_set = {
                let mut frame = frame.lock().unwrap();
                CaptureSet::new(&mut frame, &capture_map)
            };

            async move {
                let a = a(&inner, frame).await?;
                let a_iter: ReplayableIterator =
                    a.try_into().map_err(|_| Exception::NotIterable.into_err())?;

                let iter = a_iter.map(move |x| {
                    let inner = Arc::clone(&inner);
                    let b = Arc::clone(&b);
                    let mut body_frame = Frame::new(slots_needed);
                    body_frame.apply_capture(&capture_set);
                    body_frame.assign(underscore_arg_id, Ok(x));
                    let body_frame = Mutex::new(body_frame);
                    async move { b(&inner, &body_frame).await }.boxed()
                });
                Ok(Value::OutOfLine(PlaygroundValue::Iterator(iter)))
            }
            .boxed()
        }
    }

    fn visit_lambda<'a>(
        &mut self,
        name: &str,
        ParameterList { parameters, optional_parameters, variadic }: ParameterList<'a>,
        body: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let mut body_visitor = Visitor::new(self.global_fs_root.clone(), self.global_pwd.clone());
        let underscore_slot = body_visitor.allocate_slot("_", Mutability::Mutable);
        let required_params_count = parameters.len();
        let params: Vec<_> = parameters
            .iter()
            .chain(optional_parameters.iter())
            .map(|x| body_visitor.allocate_slot(*x.fragment(), Mutability::Mutable))
            .collect();
        let variadic =
            variadic.map(|x| body_visitor.allocate_slot(*x.fragment(), Mutability::Mutable));
        let body = body_visitor.visit(body);
        let slots_needed = body_visitor.slots_needed();
        let capture_map = self.capture_map_for(&body_visitor);
        let name = name.to_owned();

        move |inner, frame| {
            let body = Arc::clone(&body);
            let inner = Arc::clone(inner);
            let params = params.clone();
            let name = name.clone();
            let capture_set = {
                let mut frame = frame.lock().unwrap();
                CaptureSet::new(&mut frame, &capture_map)
            };

            async move {
                let name = name.clone();
                let capture_set = capture_set.clone();

                Ok(Value::OutOfLine(PlaygroundValue::Invocable(Invocable::new(Arc::new(
                    move |mut args, underscore| {
                        let body = Arc::clone(&body);
                        let inner = Arc::clone(&inner);
                        let params = params.clone();
                        let name = name.clone();
                        let capture_set = capture_set.clone();

                        async move {
                            let args_len = args.len();
                            if args_len < required_params_count {
                                Err(Exception::WrongArgumentCount(
                                    name,
                                    required_params_count,
                                    args_len,
                                )
                                .into())
                            } else {
                                let mut body_frame = Frame::new(slots_needed);
                                body_frame.apply_capture(&capture_set);
                                let len = std::cmp::min(params.len(), args.len());
                                for (param, arg) in params
                                    .iter()
                                    .copied()
                                    .zip(args.drain(..len).chain(repeat_with(|| Value::Null)))
                                {
                                    body_frame.assign(param, Ok(arg));
                                }

                                if !args.is_empty() {
                                    if let Some(variadic) = variadic.clone() {
                                        body_frame.assign(variadic, Ok(Value::List(args)));
                                    } else {
                                        return Err(Exception::WrongArgumentCount(
                                            name,
                                            required_params_count,
                                            args_len,
                                        )
                                        .into());
                                    }
                                }

                                body_frame
                                    .assign(underscore_slot, Ok(underscore.unwrap_or(Value::Null)));

                                body(&inner, &Mutex::new(body_frame)).await.map_err(|mut e| {
                                    e.in_func(&name);
                                    e
                                })
                            }
                        }
                        .boxed()
                    },
                )))))
            }
            .boxed()
        }
    }

    fn visit_le<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        self.binop(a, b, |a, b| {
            Ok(Value::Bool(playground_semantic_compare(&a, &b).map(|x| x.is_le()).unwrap_or(false)))
        })
    }

    fn visit_lt<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        self.binop(a, b, |a, b| {
            Ok(Value::Bool(playground_semantic_compare(&a, &b).map(|x| x.is_lt()).unwrap_or(false)))
        })
    }

    fn visit_label<'a>(
        &mut self,
        value: &str,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let value = value.to_owned();
        move |_, _| {
            let value = value.clone();
            async move { Ok(Value::String(value)) }.boxed()
        }
    }

    fn visit_list<'a>(
        &mut self,
        items: Vec<Node<'a>>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let mut item_instructions = Vec::new();

        for item in items.into_iter() {
            item_instructions.push(self.visit(item));
        }

        let item_instructions = Arc::new(item_instructions);
        move |inner, frame| {
            let item_instructions = Arc::clone(&item_instructions);
            let inner = Arc::clone(inner);

            async move {
                let mut items = Vec::new();
                for item in item_instructions.iter() {
                    let item = item(&inner, frame).await?;
                    items.push(item);
                }

                Ok(Value::List(items))
            }
            .boxed()
        }
    }

    fn visit_logical_and_or<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
        op_name: &'static str,
        is_or: bool,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let a = self.visit(a);
        let b = self.visit(b);

        move |inner, frame| {
            let a = Arc::clone(&a);
            let b = Arc::clone(&b);
            let inner = Arc::clone(inner);

            async move {
                let a = a(&inner, frame).await?;

                match a {
                    Value::Bool(x) if x == is_or => Ok(a),
                    _ => {
                        let b = b(&inner, frame).await?;

                        match (&a, &b) {
                            (Value::Bool(_), Value::Bool(_)) => Ok(b),
                            _ => Err(Exception::BadBooleanOperands(op_name).into()),
                        }
                    }
                }
            }
            .boxed()
        }
    }

    fn visit_logical_not<'a>(
        &mut self,
        a: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let a = self.visit(a);

        move |inner, frame| {
            let a = Arc::clone(&a);
            let inner = Arc::clone(inner);

            async move {
                let a = a(&inner, frame).await?;

                match a {
                    Value::Bool(a) => Ok(Value::Bool(!a)),
                    _ => Err(Exception::BadBooleanOperands("!").into()),
                }
            }
            .boxed()
        }
    }

    fn visit_lookup<'a>(
        &mut self,
        target: Node<'a>,
        key: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let target = self.visit(target);
        let key = self.visit(key);
        move |inner, frame| {
            let target = Arc::clone(&target);
            let key = Arc::clone(&key);
            let inner = Arc::clone(inner);
            async move {
                let key = key(&inner, frame).await?;

                match target(&inner, frame).await? {
                    Value::List(mut l) => {
                        let key: usize = key
                            .try_usize()
                            .map_err(|_| Exception::NonPositiveIntegerListKey.into_err())?;

                        if key < l.len() {
                            Ok(l.swap_remove(key))
                        } else {
                            Err(Exception::ListIndexOutOfRange.into())
                        }
                    }
                    Value::Object(s) => {
                        let Value::String(key) = key else {
                            return Err(Exception::NonStringObjectKey.into());
                        };

                        s.into_iter()
                            .find(|(k, _)| *k == key)
                            .map(|(_, x)| x)
                            .ok_or_else(|| Exception::NoSuchObjectKey(key).into_err())
                    }
                    Value::Union(_ty, field, v) => {
                        if let Value::String(key) = key {
                            if key == field {
                                Ok(*v)
                            } else {
                                Err(Exception::NoSuchObjectKey(key).into())
                            }
                        } else if let Ok(key) = key.try_usize() {
                            if key == 0 {
                                Ok(*v)
                            } else {
                                Err(Exception::ListIndexOutOfRange.into())
                            }
                        } else {
                            Err(Exception::BadUnionKey.into())
                        }
                    }
                    _ => Err(Exception::LookupNotSupported.into()),
                }
            }
            .boxed()
        }
    }

    fn visit_multiply<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        self.binop(a, b, |a, b| {
            try_numeric_math(a, b, |a, b| Ok(Value::OutOfLine(PlaygroundValue::Num(a * b))))
                .unwrap_or_else(|| Err(Exception::BadNumericOperands("*").into()))
        })
    }

    fn visit_ne<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        self.binop(a, b, |a, b| {
            Ok(Value::Bool(playground_semantic_compare(&a, &b).map(|x| x.is_ne()).unwrap_or(false)))
        })
    }

    fn visit_negate<'a>(
        &mut self,
        a: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let a = self.visit(a);

        move |inner, frame| {
            let a = Arc::clone(&a);
            let inner = Arc::clone(inner);

            async move {
                let a = a(&inner, frame).await?;

                a.try_big_num()
                    .map(|x: BigRational| Value::OutOfLine(PlaygroundValue::Num(-x)))
                    .map_err(|_| Exception::BadNegationOperand.into())
            }
            .boxed()
        }
    }

    fn visit_object<'a>(
        &mut self,
        label: Option<Span<'a>>,
        values: Vec<(Node<'a>, Node<'a>)>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let name = label.map(|x| (*x.fragment()).to_owned());
        enum KeyType<A> {
            Literal(String),
            Expr(A),
        }
        let values = values
            .into_iter()
            .map(|(x, y)| {
                (
                    match x {
                        Node::Identifier(s) => KeyType::Literal((*s.fragment()).to_owned()),
                        x => KeyType::Expr(self.visit(x)),
                    },
                    self.visit(y),
                )
            })
            .collect::<Arc<[_]>>();

        move |inner, frame| {
            let values = Arc::clone(&values);
            let inner = Arc::clone(inner);
            let name = name.clone();

            async move {
                let mut object = HashMap::new();

                for (k, v) in values.iter() {
                    let k = match k {
                        KeyType::Literal(s) => s.clone(),
                        KeyType::Expr(k) => {
                            let Value::String(s) = k(&inner, frame).await? else {
                                panic!("Parser didn't give a string literal for object key");
                            };
                            s
                        }
                    };

                    let v = v(&inner, frame).await?;
                    object.insert(k, v);
                }

                let value = Value::Object(object.into_iter().collect());
                if let Some(name) = name {
                    Ok(Value::OutOfLine(PlaygroundValue::TypeHinted(name, Box::new(value))))
                } else {
                    Ok(value)
                }
            }
            .boxed()
        }
    }

    fn visit_pipe<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        self.enter_scope();
        let a_decl = Arc::new(self.visit_variable_decl_async("_", a, Mutability::Constant));
        let b = self.visit(b);
        self.exit_scope();

        move |inner, frame| {
            let b = Arc::clone(&b);
            let inner = Arc::clone(inner);

            a_decl(&inner, frame);
            async move {
                let ret = b(&inner, frame).await;
                ret
            }
            .boxed()
        }
    }

    fn visit_program<'a>(
        &mut self,
        nodes: Vec<Node<'a>>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        // We split the run of statements in the program into three sets:
        // * Some statements, none of which are imports.
        // * Some import statements.
        // * Some more statements which are imports.
        //
        // The reason is we have to do some special stuff with visitors to deal
        // with the latter two parts. So the first part we'll visit in a pretty
        // normal way, and the second two parts we'll hand to a special visitor
        // which will give us a separate callback to run that portion.
        //
        // Also, note that `import foo as bar` doesn't count as an import here;
        // the `as bar` part lets us process in a simpler way, so we don't need
        // to do this little dance.
        let mut statements = Vec::new();
        let mut trailer_imports = Vec::new();
        let mut trailer_import_statements = Vec::new();

        for node in nodes.into_iter() {
            if !trailer_import_statements.is_empty() {
                trailer_import_statements.push(node);
            } else if let Node::Import(path, None) = node {
                trailer_imports.push(path)
            } else if !trailer_imports.is_empty() {
                trailer_import_statements.push(node);
            } else {
                statements.push(self.visit(node));
            }
        }

        let statements = Arc::new(statements.into_boxed_slice());
        let trailer = if !trailer_imports.is_empty() {
            Some(Arc::new(
                self.visit_program_with_imports(trailer_imports, trailer_import_statements),
            ))
        } else {
            None
        };

        move |inner, frame| {
            let statements = Arc::clone(&statements);
            let inner = Arc::clone(&inner);
            let trailer = trailer.as_ref().map(Arc::clone);

            async move {
                let mut ret = Value::Null;

                for statement in statements.iter() {
                    ret = statement(&inner, frame).await?;
                }

                if let Some(trailer) = trailer { trailer(&inner, frame).await } else { Ok(ret) }
            }
            .boxed()
        }
    }

    /// Special visitor for program nodes that begin with imports that import directly in to their scope.
    fn visit_program_with_imports<'a>(
        &mut self,
        imports: Vec<Span<'a>>,
        nodes: Vec<Node<'a>>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let mut continuation_visitor =
            Visitor::new(self.global_fs_root.clone(), self.global_pwd.clone());
        let program = Arc::new(continuation_visitor.visit(Node::Program(nodes)));
        let imports = imports.into_iter().map(|x| (*x.fragment()).to_owned()).collect::<Vec<_>>();

        let capture_map = self.capture_map_for(&continuation_visitor);
        let slots_needed = continuation_visitor.slots_needed();
        let (captured_ids, allocated_ids) = continuation_visitor.into_slot_data();

        let fs_root = self.global_fs_root.clone().unwrap_or_else(|| self.fetch_slot("fs_root").0);
        let pwd = self.global_pwd.clone().unwrap_or_else(|| self.fetch_slot("pwd").0);

        move |inner, frame| {
            let program = Arc::clone(&program);
            let inner = Arc::clone(&inner);
            let imports = imports.clone();
            let mut captured_ids = captured_ids.clone();
            let allocated_ids = allocated_ids.clone();
            let capture_map = capture_map.clone();

            async move {
                // TODO: If we import in a nested block this will do the whole
                // import including file loading every single time we loop
                // through that block. Not ideal?
                let (fs_root, pwd) = {
                    let frame = frame.lock().unwrap();
                    (frame.get(fs_root), frame.get(pwd))
                };

                let mut fs_root = fs_root.await?;
                let mut pwd = pwd.await?;

                let mut globals: Option<crate::frame::GlobalVariables> = None;
                for import in imports {
                    let interpreter = Interpreter::new_with_inner(
                        Arc::clone(&inner),
                        fs_root.duplicate(),
                        pwd.duplicate(),
                    )
                    .await;
                    let new_globals = interpreter.run_isolated_import(import).await?;

                    if let Some(globals) = &mut globals {
                        globals.merge(new_globals);
                    } else {
                        globals = Some(new_globals);
                    }
                }

                let globals = globals.unwrap();

                let capture_set = {
                    let mut frame = frame.lock().unwrap();
                    CaptureSet::new(&mut frame, &capture_map)
                };

                let mut continuation_frame = Frame::new(slots_needed);
                continuation_frame.apply_capture(&capture_set);
                globals.apply_to_frame(&mut continuation_frame, |ident| {
                    if let Some(id) = captured_ids.remove(ident) {
                        Some(id)
                    } else {
                        allocated_ids.get(ident).copied()
                    }
                });
                program(&inner, &Mutex::new(continuation_frame)).await
            }
            .boxed()
        }
    }

    fn visit_range<'a>(
        &mut self,
        a: Option<Node<'a>>,
        b: Option<Node<'a>>,
        is_inclusive: bool,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let a = a.map(|a| self.visit(a));
        let b = b.map(|b| self.visit(b));

        move |inner, frame| {
            let a = a.clone();
            let b = b.clone();
            let inner = Arc::clone(inner);

            async move {
                let a: Option<BigInt> = if let Some(a) = a {
                    let a = a(&inner, frame).await?;
                    Some(a.try_big_int().map_err(|_| Exception::NonNumericRangeBound.into_err())?)
                } else {
                    None
                };
                let b: Option<BigInt> = if let Some(b) = b {
                    let b = b(&inner, frame).await?;
                    Some(b.try_big_int().map_err(|_| Exception::NonNumericRangeBound.into_err())?)
                } else {
                    None
                };

                let a = if let Some(a) = a {
                    a
                } else {
                    return Err(Exception::UnsupportedLeftUnboundedRange.into());
                };

                let range_cursor = RangeCursor { start: a, end: b, is_inclusive };

                Ok(Value::OutOfLine(PlaygroundValue::Iterator(range_cursor.into())))
            }
            .boxed()
        }
    }

    fn visit_real(
        &mut self,
        string: &str,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let (whole, decimal) = string.split_once('.').expect("Parser yielded invalid real!");

        let denom_size = decimal.chars().filter(|x| "0123456789".contains(*x)).count();
        let denom = BigInt::from_u8(10).unwrap().pow(
            denom_size
                .try_into()
                .expect("Tried to use a 4-million digit decimal, which seems excessive really."),
        );
        let whole = parse_integer(whole);
        let decimal = parse_integer(decimal);
        let numerator = whole * denom.clone() + decimal;
        let value = BigRational::new(numerator, denom);

        move |_, _| {
            let value = value.clone();
            async move { Ok(Value::OutOfLine(PlaygroundValue::Num(value))) }.boxed()
        }
    }

    fn visit_string(
        &mut self,
        elements: Vec<StringElement<'_>>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        enum CompiledElements<A> {
            Body(String),
            Interpolation(A),
        }

        let mut compiled_elements = Vec::with_capacity(elements.len());
        let mut string_capacity = elements.len();
        for element in elements {
            match element {
                StringElement::Body(element) => {
                    let mut element = element
                        .replace(r"\n", "\n")
                        .replace(r"\t", "\t")
                        .replace(r"\r", "\r")
                        .replace("\\\n", "")
                        .replace(r"\$", "$")
                        .replace(r#"\""#, "\"");

                    while let Some(idx) = element.find("\\u") {
                        let chr = std::char::from_u32(
                            u32::from_str_radix(&element[(idx + 2)..(idx + 8)], 16).unwrap(),
                        )
                        .unwrap_or('�');
                        element.replace_range(idx..(idx + 8), &chr.to_string());
                    }

                    let element = element.replace(r"\\", r"\");
                    string_capacity += element.len();
                    string_capacity -= 1;

                    if let Some(CompiledElements::Body(b)) = compiled_elements.last_mut() {
                        b.push_str(&element)
                    } else {
                        compiled_elements.push(CompiledElements::Body(element));
                    }
                }
                StringElement::Interpolation(i) => {
                    compiled_elements.push(CompiledElements::Interpolation(self.visit(i)))
                }
            }
        }

        let elements = Arc::new(compiled_elements);

        move |inner, frame| {
            let elements = Arc::clone(&elements);
            let inner = Arc::clone(inner);

            async move {
                let mut string = String::with_capacity(string_capacity);
                for element in elements.iter() {
                    match element {
                        CompiledElements::Body(b) => string.push_str(b),
                        CompiledElements::Interpolation(i) => {
                            let v = i(&inner, frame).await?;
                            if let Value::String(s) = &v {
                                string.push_str(s);
                            } else {
                                write!(&mut string, "{v}").expect("Write to string failed!");
                            }
                        }
                    }
                }
                Ok(Value::String(string))
            }
            .boxed()
        }
    }

    fn visit_subtract<'a>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        self.binop(a, b, |a, b| {
            try_numeric_math(a, b, |a, b| Ok(Value::OutOfLine(PlaygroundValue::Num(a - b))))
                .unwrap_or_else(|| Err(Exception::BadNumericOperands("-").into()))
        })
    }

    fn visit_function_decl<'a>(
        &mut self,
        identifier: &str,
        params: ParameterList<'a>,
        body: Node<'a>,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let lambda = Arc::new(self.visit_lambda(identifier, params, body));
        let slot = self.allocate_slot(identifier, Mutability::Constant);

        move |inner, frame| {
            let lambda = Arc::clone(&lambda);
            let inner = Arc::clone(inner);

            async move {
                let (sender, receiver) = oneshot_channel();
                let task = frame
                    .lock()
                    .unwrap()
                    .assign_future_ignore_const(slot, async move { Ok(receiver.await.unwrap()) });
                inner.push_task(task);
                sender.send(lambda(&inner, frame).await?).unwrap();
                Ok(Value::Null)
            }
            .boxed()
        }
    }

    fn visit_variable_decl(
        &mut self,
        identifier: &str,
        value: Node<'_>,
        mutability: Mutability,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>> + use<>
    {
        let slot = self.allocate_slot(identifier, mutability);
        let value = self.visit(value);

        move |inner, frame| {
            let value = Arc::clone(&value);
            let inner = Arc::clone(&inner);

            async move {
                let mut got = value(&inner, frame).await?;
                frame.lock().unwrap().assign_ignore_const(slot, Ok(got.duplicate()));
                Ok(got)
            }
            .boxed()
        }
    }

    fn visit_variable_decl_async(
        &mut self,
        identifier: &str,
        value: Node<'_>,
        mutability: Mutability,
    ) -> impl Fn(&Arc<InterpreterInner>, &Mutex<Frame>) + use<> {
        let mut value_visitor = Visitor::new(self.global_fs_root.clone(), self.global_pwd.clone());
        let value = value_visitor.visit(value);
        let capture_map = self.capture_map_for(&value_visitor);
        let slots_needed = value_visitor.slots_needed();
        let slot = self.allocate_slot(identifier, mutability);

        move |inner, frame| {
            let capture_set = {
                let mut frame = frame.lock().unwrap();
                CaptureSet::new(&mut frame, &capture_map)
            };
            let mut body_frame = Frame::new(slots_needed);
            body_frame.apply_capture(&capture_set);
            let body_frame = Mutex::new(body_frame);
            let inner_clone = Arc::clone(inner);
            let value = Arc::clone(&value);
            let value = async move { value(&inner_clone, &body_frame).await };
            let mut frame = frame.lock().unwrap();
            let task = frame.assign_future_ignore_const(slot, value);
            inner.push_task(task);
        }
    }

    fn binop<'a, Op>(
        &mut self,
        a: Node<'a>,
        b: Node<'a>,
        op: Op,
    ) -> impl for<'f> Fn(&Arc<InterpreterInner>, &'f Mutex<Frame>) -> BoxFuture<'f, Result<Value>>
    + use<Op>
    where
        Op: Fn(Value, Value) -> Result<Value> + Send + Sync + 'static,
    {
        let a = self.visit(a);
        let b = self.visit(b);
        let op = Arc::new(op);

        move |inner, frame| {
            let a = Arc::clone(&a);
            let b = Arc::clone(&b);
            let inner = Arc::clone(inner);
            let op = Arc::clone(&op);
            async move {
                let a = a(&inner, frame).await?;
                let b = b(&inner, frame).await?;
                op(a, b)
            }
            .boxed()
        }
    }
}
