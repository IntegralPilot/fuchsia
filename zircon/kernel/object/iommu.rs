// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::iommu_dispatcher_ffi::{cpp_iommu_get_ref_counted, cpp_iommu_recycle};

fbl::impl_opaque_ref_counted_facade!(
    pub struct Iommu,
    cpp_iommu_recycle,
    cpp_iommu_get_ref_counted,
);
