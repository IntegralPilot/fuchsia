// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/node_page.h"

#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/node.h"

namespace f2fs {
void NodePage::FillNodeFooter(nid_t nid, nid_t ino, size_t ofs) {
  NodeFooter &raw_footer = node().footer;
  raw_footer.nid = CpuToLe(nid);
  raw_footer.ino = CpuToLe(ino);
  raw_footer.flag = CpuToLe(
      safemath::checked_cast<uint32_t>(ofs << static_cast<uint32_t>(BitShift::kOffsetBitShift)));
}

void NodePage::CopyNodeFooterFrom(NodePage &src) {
  memcpy(&node().footer, &src.node().footer, sizeof(NodeFooter));
}

void NodePage::FillNodeFooterBlkaddr(block_t blkaddr, uint64_t ver) {
  NodeFooter &raw_footer = node().footer;
  raw_footer.cp_ver = CpuToLe(ver);
  raw_footer.next_blkaddr = CpuToLe(blkaddr);
}

nid_t NodePage::InoOfNode() const { return LeToCpu(node().footer.ino); }

nid_t NodePage::NidOfNode() const { return LeToCpu(node().footer.nid); }

uint32_t NodePage::OfsOfNode() const {
  uint32_t flag = LeToCpu(node().footer.flag);
  return flag >> static_cast<int>(BitShift::kOffsetBitShift);
}

uint64_t NodePage::CpverOfNode() const { return LeToCpu(node().footer.cp_ver); }

block_t NodePage::NextBlkaddrOfNode() const { return LeToCpu(node().footer.next_blkaddr); }

// f2fs assigns the following node offsets described as (num).
// N = kNidsPerBlock
//
//  Inode block (0)
//    |- direct node (1)
//    |- direct node (2)
//    |- indirect node (3)
//    |            `- direct node (4 => 4 + N - 1)
//    |- indirect node (4 + N)
//    |            `- direct node (5 + N => 5 + 2N - 1)
//    `- double indirect node (5 + 2N)
//                 `- indirect node (6 + 2N)
//                       `- direct node (x(N + 1))
bool NodePage::IsDnode() const {
  uint32_t ofs = OfsOfNode();
  if (ofs == kOfsIndirectNode1 || ofs == kOfsIndirectNode2 || ofs == kOfsDoubleIndirectNode) {
    return false;
  }
  if (ofs >= kOfsDoubleIndirectNode + 1) {
    ofs -= kOfsDoubleIndirectNode + 1;
    // In the double-indirect subtree, nodes appear in repeating groups of (N + 1) nodes:
    // 1 Indirect Node followed by N Direct Nodes. Therefore, if the relative offset
    // is a multiple of (N + 1), it is an Indirect Node, not a Direct Node.
    if (ofs % (kNidsPerBlock + 1) == 0) {
      return false;
    }
  }
  return true;
}

void NodePage::SetNid(size_t off, nid_t nid) {
  if (IsInode()) {
    node().i.i_nid[off - kNodeDir1Block] = CpuToLe(nid);
  } else {
    node().in.nid[off] = CpuToLe(nid);
  }
}

nid_t NodePage::GetNid(size_t off) const {
  if (IsInode()) {
    return LeToCpu(node().i.i_nid[off - kNodeDir1Block]);
  }
  return LeToCpu(node().in.nid[off]);
}

bool NodePage::IsColdNode() const {
  uint32_t flag = LeToCpu(node().footer.flag);
  uint32_t bit =
      safemath::CheckLsh(1U, static_cast<uint32_t>(BitShift::kColdBitShift)).ValueOrDie();
  return flag & bit;
}

bool NodePage::IsFsyncDnode() const {
  uint32_t flag = LeToCpu(node().footer.flag);
  uint32_t bit =
      safemath::CheckLsh(1U, static_cast<uint32_t>(BitShift::kFsyncBitShift)).ValueOrDie();
  return flag & bit;
}

bool NodePage::IsDentDnode() const {
  uint32_t flag = LeToCpu(node().footer.flag);
  uint32_t bit =
      safemath::CheckLsh(1U, static_cast<uint32_t>(BitShift::kDentBitShift)).ValueOrDie();
  return flag & bit;
}

void NodePage::SetColdNode(const bool is_dir) {
  Node &raw_node = node();
  uint32_t flag = LeToCpu(raw_node.footer.flag);
  uint32_t bit =
      safemath::CheckLsh(1U, static_cast<uint32_t>(BitShift::kColdBitShift)).ValueOrDie();
  if (is_dir) {
    flag &= ~bit;
  } else {
    flag |= bit;
  }
  raw_node.footer.flag = CpuToLe(flag);
}

void NodePage::SetFsyncMark(bool mark) {
  Node &raw_node = node();
  uint32_t flag = LeToCpu(raw_node.footer.flag);
  uint32_t bit =
      safemath::CheckLsh(1U, static_cast<uint32_t>(BitShift::kFsyncBitShift)).ValueOrDie();
  if (mark) {
    flag |= bit;
  } else {
    flag &= ~bit;
  }
  raw_node.footer.flag = CpuToLe(flag);
}

void NodePage::SetDentryMark(bool mark) {
  Node &raw_node = node();
  uint32_t flag = LeToCpu(raw_node.footer.flag);
  uint32_t bit =
      safemath::CheckLsh(1U, static_cast<uint32_t>(BitShift::kDentBitShift)).ValueOrDie();
  if (mark) {
    flag |= bit;
  } else {
    flag &= ~bit;
  }
  raw_node.footer.flag = CpuToLe(flag);
}

size_t NodePage::StartBidxOfNode(size_t num_addrs) const {
  size_t node_ofs = OfsOfNode();
  size_t num_of_indirect_nodes = 0;

  if (node_ofs == kOfsInode) {
    return 0;
  }
  if (node_ofs <= kOfsDirectNode2) {
    num_of_indirect_nodes = 0;
  } else if (node_ofs >= kOfsIndirectNode1 && node_ofs < kOfsIndirectNode2) {
    num_of_indirect_nodes = 1;
  } else if (node_ofs >= kOfsIndirectNode2 && node_ofs < kOfsDoubleIndirectNode) {
    num_of_indirect_nodes = 2;
  } else if (node_ofs == kOfsDoubleIndirectNode || node_ofs == kOfsDoubleIndirectNode + 1) {
    num_of_indirect_nodes = 3;
  } else {
    // Add 4 to account for preceding indirect nodes:
    // - 3 indirect nodes in levels 1 and 2 (kOfsIndirectNode1, kOfsIndirectNode2,
    // kOfsDoubleIndirectNode)
    // - 1 intermediate indirect node (offset kOfsDoubleIndirectNode + 1) inside this Level 3
    // subtree.
    num_of_indirect_nodes = (node_ofs - kOfsDoubleIndirectNode - 2) / (kNidsPerBlock + 1) + 4;
  }

  size_t bidx = node_ofs - num_of_indirect_nodes - 1;
  return (num_addrs + safemath::CheckMul(bidx, kAddrsPerBlock)).ValueOrDie();
}

bool NodePage::IsInode() const {
  NodeFooter &raw_footer = node().footer;
  return raw_footer.nid == raw_footer.ino;
}

// Linux f2fs indexes an inode's addresses through the vnode's already-validated i_extra_isize
// and reads the on-disk field only where no vnode exists. This method has no vnode to consult
// -- recovery and GC reach it straight from a page -- so it bounds the start it reads rather
// than trusting it.
std::span<const block_t> NodePage::addrs_array() const {
  const Node &raw_node = node();
  if (!IsInode()) {
    return {raw_node.dn.addr, kAddrsPerBlock};
  }
  const Inode &inode = raw_node.i;
  size_t start = 0;
  if (inode.i_inline & kExtraAttr) {
    // Read straight from the page, which recovery and GC reach without a vnode to reject a
    // corrupted layout first, so the start can name an entry the array does not have.
    start = LeToCpu(inode.i_extra_isize) / sizeof(uint32_t);
    if (start >= kAddrsPerInode) {
      return {};
    }
  }
  return std::span<const block_t>(inode.i_addr, kAddrsPerInode).subspan(start);
}

std::span<block_t> NodePage::addrs_array() {
  // This overload is chosen only for a NodePage that is not const, so the array it names is
  // not const either and both overloads can share the one bounds calculation.
  const std::span<const block_t> addrs = static_cast<const NodePage *>(this)->addrs_array();
  return {const_cast<block_t *>(addrs.data()), addrs.size()};
}

block_t NodePage::GetBlockAddr(const size_t offset) const {
  std::span<const block_t> addrs = addrs_array();
  if (offset >= addrs.size()) {
    FX_LOGS(WARNING) << "node " << NidOfNode() << " holds " << addrs.size()
                     << " block addresses, but offset " << offset << " was requested";
    return kNullAddr;
  }
  return LeToCpu(addrs[offset]);
}

void NodePage::SetDataBlkaddr(size_t ofs_in_node, block_t new_addr) {
  std::span<block_t> addrs = addrs_array();
  if (ofs_in_node >= addrs.size()) {
    // The array bounds come from the image, so this is a corrupted node rather than a caller
    // that got its arithmetic wrong. Drop the update instead of writing outside the block.
    FX_LOGS(WARNING) << "node " << NidOfNode() << " holds " << addrs.size()
                     << " block addresses, so " << new_addr << " cannot be recorded at "
                     << ofs_in_node;
    return;
  }

  // A newly reserved block may only take the place of a hole, and any other address may only
  // replace one that is already there. Violating that is a caller bug, not a corrupted image.
  const block_t old_addr = LeToCpu(addrs[ofs_in_node]);
  const bool replaces_hole_iff_new = (new_addr == kNewAddr) == (old_addr == kNullAddr);
  if (!replaces_hole_iff_new) {
    FX_LOGS(WARNING) << "node " << NidOfNode() << " records " << new_addr << " at " << ofs_in_node
                     << " over " << old_addr;
    ZX_DEBUG_ASSERT(replaces_hole_iff_new);
  }

  addrs[ofs_in_node] = CpuToLe(new_addr);
}

}  // namespace f2fs
