// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::{Error, Inner, InnerType, InspectType, State};
use inspect_format::BlockIndex;

/// Inspect Lazy Node data type.
/// NOTE: do not rely on PartialEq implementation for true comparison.
/// Instead leverage the reader.
///
/// NOTE: Operations on a Default value are no-ops.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct LazyNode {
    inner: Inner<InnerLazyNodeType>,
}

impl InspectType for LazyNode {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType {
        crate::writer::types::RecordedInspectType::LazyNode(self)
    }
}

crate::impl_inspect_type_internal!(LazyNode);

#[derive(Default, Debug)]
struct InnerLazyNodeType;

impl InnerType for InnerLazyNodeType {
    type Data = ();
    fn free(state: &State, _: &Self::Data, block_index: BlockIndex) -> Result<(), Error> {
        let mut state_lock = state.try_lock()?;
        state_lock
            .free_lazy_node(block_index)
            .map_err(|err| Error::free("lazy node", block_index, err))
    }
}

#[cfg(test)]
mod tests {
    use crate::writer::testing_utils::GetBlockExt;
    use crate::writer::types::Inspector;
    use futures::FutureExt;
    use inspect_format::{BlockType, Link, LinkNodeDisposition};

    #[fuchsia::test]
    fn lazy_values() {
        let inspector = Inspector::default();
        let node = inspector.root().create_child("node");
        {
            let lazy_node =
                node.create_lazy_values("lazy", || async move { Ok(Inspector::default()) }.boxed());
            lazy_node.get_block::<_, Link>(|lazy_node_block| {
                assert_eq!(lazy_node_block.block_type(), Some(BlockType::LinkValue));
                assert_eq!(
                    lazy_node_block.link_node_disposition(),
                    Some(LinkNodeDisposition::Inline)
                );
                assert_eq!(*lazy_node_block.content_index(), 6);
            });
            node.get_block::<_, inspect_format::Node>(|node_block| {
                assert_eq!(node_block.child_count(), 1);
            });
        }
        node.get_block::<_, inspect_format::Node>(|node_block| {
            assert_eq!(node_block.child_count(), 0);
        });
    }

    #[fuchsia::test]
    fn lazy_node() {
        let inspector = Inspector::default();
        let node = inspector.root().create_child("node");
        {
            let lazy_node =
                node.create_lazy_child("lazy", || async move { Ok(Inspector::default()) }.boxed());
            lazy_node.get_block::<_, Link>(|lazy_node_block| {
                assert_eq!(lazy_node_block.block_type(), Some(BlockType::LinkValue));
                assert_eq!(
                    lazy_node_block.link_node_disposition(),
                    Some(LinkNodeDisposition::Child)
                );
                assert_eq!(*lazy_node_block.content_index(), 6);
            });
            node.get_block::<_, inspect_format::Node>(|node_block| {
                assert_eq!(node_block.child_count(), 1);
            });
        }
        node.get_block::<_, inspect_format::Node>(|node_block| {
            assert_eq!(node_block.child_count(), 0);
        });
    }
}
