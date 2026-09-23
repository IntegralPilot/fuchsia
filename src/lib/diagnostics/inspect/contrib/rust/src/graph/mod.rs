// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//! # Inspect Graph
//!
//! This module provides an abstraction over a Directed Graph on Inspect.
//!
//! The graph has vertices and edges. Edges have an origin and destination vertex.
//! Each vertex and edge can have a set of key value pairs of associated metadata.
//!
//! The resulting graph has the following schema:
//!
//! {
//!     "fuchsia.inspect.Graph": {
//!         "topology": {
//!             "vertex-0": {
//!                 "meta": {
//!                     "key-1": value,
//!                     ...
//!                     "key-i": value
//!                 },
//!                 "relationships": {
//!                     "vertex-j": {
//!                         "meta": {
//!                             "key-1": value,
//!                             ...
//!                             "key-i": value
//!                         }
//!                     },
//!                     ...
//!                     "vertex-k": {
//!                         "meta": { ... },
//!                     },
//!                 }
//!             },
//!             ...
//!             "vertex-i": {
//!                 "meta": { ...  },
//!                 "relationships": { ... },
//!             }
//!         }
//!     }
//! }
//!
//! The `topology` node contains all the vertices as children, each of the child names is the ID
//! of the vertex provided through the API.
//!
//! Each vertex has a metadata associated with it under the child `meta`. Each of the child names
//! of meta is the key of the metadata field.
//!
//! Each vertex also has a child `relationships` which contains all the outgoing edges of that
//! vertex. Each edge is identified by an incremental ID assigned at runtime and contains a property
//! `@to` which represents the vertex that has that incoming edge. Similar to vertices, it also
//! has a `meta` containing metadata key value pairs.
//!
//! ## Semantics
//!
//! This API follows regular inspect semantics with the following important detail: Dropping a
//! Vertex results in the deletion of all the associated metadata as well as all the associated
//! outgoing and incoming edges from the Inspect VMO. This is especially important for Edges
//! given that the program may still be holding an Edge struct, but if any of the nodes associated
//! with that edge is dropped, the Edge data will be considered as removed from the Inspect VMO and
//! operations on the Edge will be no-ops.
//!
//! ## Overview of type structure
//!
//! GraphMetadata is the key-value storage per inspect Node; the value's type is MetadataProperty.
//! VertexGraphMetadata and EdgeGraphMetadata each contain a field, inner, of type GraphMetadata.
//!
//! Metadata is used as input specification but not storage. It holds key and value;
//! the value is an InnerMetadata enum with internal types MetadataValue and boolean, or Nested.

mod digraph;
mod edge;
mod types;
mod vertex;

pub use digraph::{Digraph, DigraphOpts};
pub use edge::{Edge, EdgeMetadata};
pub use types::VertexId;
pub use vertex::{Vertex, VertexMetadata};
