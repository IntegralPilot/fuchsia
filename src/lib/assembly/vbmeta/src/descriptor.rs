// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod chain_partition;
pub mod hash;
pub mod hash_builder;
pub mod kernel_cmdline;
pub mod property;

pub use chain_partition::ChainPartitionDescriptor;
pub use hash::{HashDescriptor, Salt, SaltError};
pub use hash_builder::HashDescriptorBuilder;
pub use kernel_cmdline::KernelCmdlineDescriptor;
pub use property::PropertyDescriptor;

/// A VBMeta descriptor.
#[derive(Clone, Debug, PartialEq)]
pub enum Descriptor {
    /// Property descriptor.
    Property(PropertyDescriptor),
    /// Hash descriptor.
    Hash(HashDescriptor),
    /// Kernel command line descriptor.
    KernelCmdline(KernelCmdlineDescriptor),
    /// Chain partition descriptor.
    ChainPartition(ChainPartitionDescriptor),
}

impl Descriptor {
    /// Serialize the Descriptor in the format expected by VBMeta.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::Property(prop) => prop.to_bytes(),
            Self::Hash(hash) => hash.to_bytes(),
            Self::KernelCmdline(cmdline) => cmdline.to_bytes(),
            Self::ChainPartition(chain) => chain.to_bytes(),
        }
    }
}
