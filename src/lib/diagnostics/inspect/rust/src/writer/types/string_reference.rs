// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::borrow::{Borrow, Cow};
use std::ops::Deref;
use std::sync::Arc;

/// StringReference is a type that can be constructed and passed into
/// the Inspect API as a name of a Node. If this is done, only one
/// reference counted instance of the string will be allocated per
/// Inspector. They can be safely used with LazyNodes.
///
/// StringReference dereferences into a `&str` for convenience.
#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub struct StringReference(Arc<str>);

impl Deref for StringReference {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl Borrow<str> for StringReference {
    fn borrow(&self) -> &str {
        self.as_ref()
    }
}

impl AsRef<str> for StringReference {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

/// Internally clones itself without allocation or copy
impl From<&'_ StringReference> for StringReference {
    fn from(sref: &'_ StringReference) -> Self {
        sref.clone()
    }
}

/// Consumes the input without allocation or copy
impl From<String> for StringReference {
    fn from(data: String) -> Self {
        StringReference(Arc::from(data.into_boxed_str()))
    }
}

/// Consumes the input without allocation or copy
impl From<Arc<str>> for StringReference {
    fn from(data: Arc<str>) -> Self {
        StringReference(data)
    }
}

/// Allocates an `Arc<String>` from `data`
impl From<&String> for StringReference {
    fn from(data: &String) -> Self {
        StringReference(Arc::from(data.as_ref()))
    }
}

/// Allocates an `Arc<String>` from `data`
impl From<&str> for StringReference {
    fn from(data: &str) -> Self {
        StringReference(data.into())
    }
}

/// Allocates an `Arc<String>` from `data`
impl From<Cow<'_, str>> for StringReference {
    fn from(data: Cow<'_, str>) -> Self {
        StringReference(data.as_ref().into())
    }
}

#[cfg(test)]
mod tests {
    use crate::writer::testing_utils::{GetBlockExt, get_state};
    use crate::writer::{Inspector, Node};
    use diagnostics_assertions::assert_data_tree;

    #[fuchsia::test]
    async fn string_references_as_names() {
        let inspector = Inspector::default();
        inspector.root().record_int("foo", 0);
        let child = inspector.root().create_child("bar");
        child.record_double("foo", 3.25);

        assert_data_tree!(inspector, root: {
            foo: 0i64,
            bar: {
                foo: 3.25,
            },
        });

        {
            let _baz_property = child.create_uint("baz", 4);
            assert_data_tree!(inspector, root: {
                foo: 0i64,
                bar: {
                    baz: 4u64,
                    foo: 3.25,
                },
            });
        }

        assert_data_tree!(inspector, root: {
            foo: 0i64,
            bar: {
                foo: 3.25,
            },
        });

        let pre_loop_allocated =
            inspector.state().unwrap().try_lock().unwrap().stats().allocated_blocks;
        let pre_loop_deallocated =
            inspector.state().unwrap().try_lock().unwrap().stats().deallocated_blocks;

        for i in 0..300 {
            child.record_int("bar", i);
        }

        assert_eq!(
            inspector.state().unwrap().try_lock().unwrap().stats().allocated_blocks,
            pre_loop_allocated + 300
        );
        assert_eq!(
            inspector.state().unwrap().try_lock().unwrap().stats().deallocated_blocks,
            pre_loop_deallocated
        );

        let pre_loop_count = pre_loop_allocated + 300;

        for i in 0..300 {
            child.record_int("abcd", i);
        }

        assert_eq!(
            inspector.state().unwrap().try_lock().unwrap().stats().allocated_blocks,
            pre_loop_count + 300 /* the int blocks */ + 1 /* individual block for "abcd" */
        );
        assert_eq!(
            inspector.state().unwrap().try_lock().unwrap().stats().deallocated_blocks,
            pre_loop_deallocated
        );
    }

    #[fuchsia::test]
    fn owned_method_argument_properties() {
        let state = get_state(4096);
        let root = Node::new_root(state);
        let node = root.create_child("node");
        {
            let _string_property =
                node.create_string(String::from("string_property"), String::from("test"));
            let _bytes_property =
                node.create_bytes(String::from("bytes_property"), vec![0, 1, 2, 3]);
            let _double_property = node.create_double(String::from("double_property"), 1.0);
            let _int_property = node.create_int(String::from("int_property"), 1);
            let _uint_property = node.create_uint(String::from("uint_property"), 1);
            node.get_block::<_, inspect_format::Node>(|node_block| {
                assert_eq!(node_block.child_count(), 5);
            });
        }
        node.get_block::<_, inspect_format::Node>(|node_block| {
            assert_eq!(node_block.child_count(), 0);
        });
    }
}
