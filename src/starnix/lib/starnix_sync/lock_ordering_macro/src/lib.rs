// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use syn::Ident;

#[derive(Clone, PartialEq, Eq, Ord, PartialOrd)]
struct Edge {
    from: Ident,
    to: Ident,
}

impl syn::parse::Parse for Edge {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let from = input.parse::<syn::Ident>()?;
        input.parse::<syn::Token![=>]>()?;
        let to: syn::Ident = input.parse()?;
        let _ = input.parse::<syn::Token![,]>();
        Ok(Edge { from, to })
    }
}

struct Graph {
    // Represents all the lock levels defined in the graph. This is a superset of `terminals`.
    // We use a BTreeSet to guarantee deterministic lock ID generation order.
    levels: BTreeSet<Ident>,
    edges: BTreeSet<Edge>,
    // Locks that are marked as terminal (i.e. having no outgoing dependencies).
    // These locks will share the maximal lock level ID.
    terminals: HashSet<Ident>,
}

impl Graph {
    fn in_degrees(&self) -> BTreeMap<Ident, usize> {
        let mut in_degrees: BTreeMap<Ident, usize> =
            self.levels.iter().map(|l| (l.clone(), 0)).collect();
        for Edge { to, .. } in self.edges.iter() {
            *in_degrees.get_mut(to).unwrap() += 1;
        }
        in_degrees
    }
}

impl syn::parse::Parse for Graph {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut levels = BTreeSet::new();
        let mut edges = BTreeSet::new();
        let mut terminals = HashSet::new();
        while !input.is_empty() {
            // We are parsing an entry of the form `Terminal(<lock_name>)`
            if input.peek(syn::Ident) && input.peek2(syn::token::Paren) {
                let ident: syn::Ident = input.parse()?;
                if ident != "Terminal" {
                    return Err(syn::Error::new(ident.span(), "Expected 'Terminal'"));
                }
                let content;
                syn::parenthesized!(content in input);
                let lock_name: syn::Ident = content.parse()?;
                let _ = input.parse::<syn::Token![,]>();
                terminals.insert(lock_name.clone());
                levels.insert(lock_name);
            } else {
                let edge: Edge = input.parse()?;
                let Edge { from, to } = edge.clone();
                levels.insert(to);
                levels.insert(from);
                edges.insert(edge);
            }
        }

        for Edge { from, .. } in edges.iter() {
            if terminals.contains(from) {
                panic!("Terminal lock {} cannot have dependencies starting from it.", from);
            }
        }

        Ok(Self { levels, edges, terminals })
    }
}

/// This macro takes a definition of the lock ordering graph in the form of
/// lock_ordering!{
///     Unlocked -> A,
///     A -> B,
///     Unlocked -> C,
/// }
///
/// and defines the edges as `LockLevel`.
#[proc_macro]
pub fn lock_ordering(input: TokenStream) -> TokenStream {
    let graph = syn::parse_macro_input!(input as Graph);
    let levels = &graph.levels;
    let edges = &graph.edges;
    let terminals = &graph.terminals;
    let mut adj_list: BTreeMap<Ident, BTreeSet<Ident>> = BTreeMap::new();

    let mut result = proc_macro2::TokenStream::new();
    for level in levels.iter() {
        adj_list.insert(level.clone(), BTreeSet::new());
    }
    for Edge { from, to } in edges.iter() {
        adj_list
            .get_mut(&from)
            .expect("Unexpected level in lock leveling graph")
            .insert(to.clone());
    }

    let mut in_degree = graph.in_degrees();

    let mut queue: std::collections::BTreeSet<Ident> = in_degree
        .iter()
        .filter_map(|(k, &v)| if v == 0 { Some(k.clone()) } else { None })
        .collect();

    let mut next_id: usize = 0;
    let mut lock_ids: BTreeMap<Ident, usize> = BTreeMap::new();
    let max_id = usize::MAX & !0xF; // Max value with subclass bits cleared

    while let Some(node) = queue.pop_first() {
        if terminals.contains(&node) {
            lock_ids.insert(node.clone(), max_id);
        } else {
            lock_ids.insert(node.clone(), next_id);
            // Space out IDs by 16 (4 bits) for subclassing
            next_id += 16;
        }
        if let Some(neighbors) = adj_list.get(&node) {
            for neighbor in neighbors {
                let deg = in_degree.get_mut(neighbor).unwrap();
                assert!(*deg > 0);
                *deg -= 1;
                if *deg == 0 {
                    queue.insert(neighbor.clone());
                }
            }
        }
    }

    if lock_ids.len() != levels.len() {
        let unprocessed: Vec<_> = levels
            .iter()
            .filter(|level| !lock_ids.contains_key(*level))
            .map(|id| id.to_string())
            .collect();
        panic!("Cycle detected in the lock ordering graph involving: {:?}", unprocessed);
    }

    for (level, id) in lock_ids {
        let name = level.to_string();
        result.extend(quote::quote! {
            pub enum #level {}
            // SAFETY: Lock level types are empty enums with no fields or Drop side-effects.
            unsafe impl fuchsia_rcu::RcuDroppable for #level {}
            impl starnix_sync::LockLevel for #level {
                const LOCK_ID: usize = #id;
                const NAME: &'static str = #name;
            }
        });
    }

    result.into()
}
