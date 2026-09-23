// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]

//! Fake BTI for testing without access to hardware resources

use anyhow::{Error, anyhow};
use std::collections::BTreeMap;
use std::ops::{Deref, Range};
use std::sync::Arc;
use zerocopy::{FromBytes, Immutable, IntoBytes};
use zx::sys::{zx_handle_t, zx_paddr_t};

mod ffi;

/// A collection of pinned VMOs returned by [`FakeBti::get_pinned_vmo`].
pub struct FakeBtiPinnedVmos {
    map: BTreeMap<usize, (Arc<zx::Vmo>, Range<u64>)>,
}

impl FakeBtiPinnedVmos {
    /// Looks up a physical address in the pinned VMOs.
    ///
    /// Returns the VMO and the range of offsets within that VMO starting at the offset
    /// corresponding to `physical_address` and ending at the end of the contiguous pinned block.
    pub fn find_phys(&self, physical_address: usize) -> Option<(Arc<zx::Vmo>, Range<u64>)> {
        let (&start_paddr, (vmo, offset_range)) =
            self.map.range(..=physical_address).next_back()?;
        let diff = (physical_address - start_paddr) as u64;
        let len = offset_range.end - offset_range.start;
        if diff < len {
            Some((vmo.clone(), (offset_range.start + diff)..offset_range.end))
        } else {
            None
        }
    }

    /// Reads bytes from physical memory.
    ///
    /// The read can span across multiple non-contiguous pinned ranges.
    pub fn read_bytes(&self, mut phys_addr: usize, len: usize) -> Result<Vec<u8>, Error> {
        let mut buf = vec![0u8; len];
        let mut read_offset = 0;

        while read_offset < len {
            let (vmo, range) = self
                .find_phys(phys_addr)
                .ok_or_else(|| anyhow!("Physical address {phys_addr:#x} not pinned"))?;
            let available = (range.end - range.start) as usize;
            let to_read = std::cmp::min(available, len - read_offset);
            vmo.read(&mut buf[read_offset..read_offset + to_read], range.start)?;
            read_offset += to_read;
            phys_addr += to_read;
        }

        Ok(buf)
    }

    /// Reads T from physical memory.
    pub fn read<T: FromBytes>(&self, phys_addr: usize) -> Result<T, Error> {
        Ok(T::read_from_bytes(&self.read_bytes(phys_addr, std::mem::size_of::<T>())?).unwrap())
    }

    /// Writes bytes to physical memory.
    ///
    /// The write can span across multiple non-contiguous pinned ranges.
    pub fn write_bytes(&self, mut phys_addr: usize, mut data: &[u8]) -> Result<(), Error> {
        while !data.is_empty() {
            let (vmo, range) = self
                .find_phys(phys_addr)
                .ok_or_else(|| anyhow!("Physical address {phys_addr:#x} not pinned"))?;
            let amount = std::cmp::min(data.len(), (range.end - range.start) as usize);
            vmo.write(data.split_off(..amount).unwrap(), range.start)?;
            phys_addr += amount;
        }
        Ok(())
    }

    /// Writes T to physical memory.
    pub fn write<T: IntoBytes + Immutable>(&self, phys_addr: usize, value: T) -> Result<(), Error> {
        self.write_bytes(phys_addr, value.as_bytes())
    }
}

/// A fake BTI object which can be created without having access to hardware resources.
#[derive(Debug)]
pub struct FakeBti(zx::Bti);

impl Deref for FakeBti {
    type Target = zx::Bti;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FakeBti {
    /// All physical addresses returned by zx_bti_pin with a fake BTI will be set to this value.
    #[inline(always)]
    pub fn phys_addr() -> usize {
        // SAFETY: This is just a constant with static storage.
        unsafe { ffi::g_fake_bti_phys_addr }
    }

    /// Creates a new FakeBti.
    pub fn create() -> Result<Self, zx::Status> {
        let handle = {
            let mut raw_handle = zx_handle_t::default();
            // SAFETY: `raw_handle` is valid
            unsafe {
                zx::ok(ffi::fake_bti_create(&mut raw_handle))?;
            }
            // SAFETY: `fake_bti_create` returned a valid handle on success
            unsafe { zx::NullableHandle::from_raw(raw_handle) }
        };
        Ok(Self(handle.into()))
    }

    /// Sets the paddrs used by the fake BTI.  Whenever [`zx::Bti::pin`] is called, these static
    /// paddrs will be written out into the resulting array.  If more physical addresses are needed
    /// than are available in paddrs, `pin` will return an error.
    pub fn set_paddrs(&self, paddrs: &[zx_paddr_t]) {
        // SAFETY: `paddrs` is valid
        unsafe {
            // OK to unwrap -- we know that `self.0` is a valid handle to a fake BTI
            zx::ok(ffi::fake_bti_set_paddrs(self.0.raw_handle(), paddrs.as_ptr(), paddrs.len()))
                .unwrap();
        }
    }

    /// Retrieves the mappings for all pinned VMOs.
    pub fn get_pinned_vmo(&self) -> FakeBtiPinnedVmos {
        let mut actual = 64;
        let vmo_infos = loop {
            let mut vmo_infos = Vec::with_capacity(actual);
            // SAFETY: `vmo_infos` has enough capacity
            unsafe {
                zx::ok(ffi::fake_bti_get_pinned_vmo(
                    self.0.raw_handle(),
                    vmo_infos.as_mut_ptr(),
                    vmo_infos.capacity(),
                    &mut actual as *mut usize,
                ))
                .unwrap();

                if actual <= vmo_infos.capacity() {
                    vmo_infos.set_len(actual);
                    break vmo_infos;
                }
                vmo_infos.set_len(vmo_infos.capacity());
                // Release all the vmos.
                for info in vmo_infos {
                    zx::NullableHandle::from_raw(info.vmo);
                }
                actual += 16; // Add some spare.
            }
        };

        let page_size = zx::system_get_page_size() as usize;
        let mut map = BTreeMap::new();
        for info in vmo_infos {
            let mut actual_paddrs = 64;
            let paddrs = loop {
                let mut paddrs = Vec::with_capacity(actual_paddrs);
                // SAFETY: `paddrs` has enough capacity
                unsafe {
                    match zx::ok(ffi::fake_bti_get_vmo_phys_address(
                        self.0.raw_handle(),
                        &info,
                        paddrs.as_mut_ptr(),
                        paddrs.capacity(),
                        &mut actual_paddrs,
                    )) {
                        Ok(()) => {}
                        Err(zx::Status::NOT_FOUND) => {
                            // This is possible due to races: the VMO could have been unpinned since
                            // we asked for all the VMOs above.
                            break vec![];
                        }
                        Err(status) => panic!("Unexpected error: {status:?}"),
                    }
                    if actual_paddrs <= paddrs.capacity() {
                        paddrs.set_len(actual_paddrs);
                        break paddrs;
                    }
                    actual_paddrs += 16; // Add some spare.
                }
            };

            // SAFETY: The handle returned by the FFI is owned by us now.
            let vmo: Arc<zx::Vmo> =
                Arc::new(unsafe { zx::NullableHandle::from_raw(info.vmo).into() });

            let mut paddrs = paddrs.into_iter();
            if let Some(a) = paddrs.next() {
                let mut offset = 0;
                let mut range = a..a + page_size;
                for a in paddrs {
                    if a == range.end {
                        range.end += page_size;
                    } else {
                        let len = (range.end - range.start) as u64;
                        map.insert(range.start, (vmo.clone(), offset..offset + len));
                        offset += len;
                        range = a..a + page_size;
                    }
                }
                let len = (range.end - range.start) as u64;
                map.insert(range.start, (vmo, offset..offset + len));
            }
        }

        FakeBtiPinnedVmos { map }
    }
}

impl From<zx::Bti> for FakeBti {
    fn from(bti: zx::Bti) -> Self {
        Self(bti)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn create() {
        let bti = FakeBti::create().expect("create failed");
        let info = bti.info().expect("info failed");
        assert!(info.minimum_contiguity > 0);
    }

    #[fuchsia::test]
    fn pin_vmo() {
        let bti = FakeBti::create().expect("create failed");
        let vmo = zx::Vmo::create(16384).expect("create VMO failed");
        let mut paddrs = [zx_paddr_t::default(); 4];
        let _pmt = bti
            .pin(zx::BtiOptions::PERM_READ, &vmo, 0, 16384, &mut paddrs[..])
            .expect("pin failed");

        let expected = FakeBti::phys_addr();
        assert_eq!(paddrs, [expected, expected, expected, expected]);
    }

    #[fuchsia::test]
    fn create_and_pin_contiguous_vmo() {
        let bti = FakeBti::create().expect("create failed");
        let vmo = zx::Vmo::create_contiguous(&bti, 16384, 0).expect("create contiguous VMO failed");
        let mut paddr = [zx_paddr_t::default()];
        let _pmt = bti
            .pin(
                zx::BtiOptions::PERM_READ | zx::BtiOptions::CONTIGUOUS,
                &vmo,
                0,
                16384,
                &mut paddr[..],
            )
            .expect("pin failed");

        assert_eq!(paddr, [FakeBti::phys_addr()]);
    }

    #[fuchsia::test]
    fn pin_with_paddrs() {
        let bti = FakeBti::create().expect("create failed");
        bti.set_paddrs(&[4096, 16384, 65536]);
        let vmo = zx::Vmo::create(3 * 4096).expect("create VMO failed");
        {
            let mut paddrs = [zx_paddr_t::default(); 3];
            let _pmt = bti
                .pin(zx::BtiOptions::PERM_READ, &vmo, 0, 3 * 4096, &mut paddrs[..])
                .expect("pin failed");
            assert_eq!(paddrs, [4096, 16384, 65536]);
        }
        bti.set_paddrs(&[4096, 8192]);
        {
            let mut paddrs = [zx_paddr_t::default(); 2];
            let _pmt = bti
                .pin(zx::BtiOptions::PERM_READ, &vmo, 0, 2 * 4096, &mut paddrs[..])
                .expect("pin failed");
            assert_eq!(paddrs, [4096, 8192]);
        }
    }

    #[fuchsia::test]
    fn test_get_pinned_vmo() {
        let bti = FakeBti::create().expect("create failed");
        bti.set_paddrs(&[0x1000, 0x2000, 0x3000, 0x4000]);

        let vmo1 = zx::Vmo::create(0x2000).expect("create VMO1 failed");
        let mut paddrs1 = [zx_paddr_t::default(); 2];
        let _pmt1 = bti
            .pin(zx::BtiOptions::PERM_READ, &vmo1, 0, 0x2000, &mut paddrs1[..])
            .expect("pin1 failed");

        let vmo2 = zx::Vmo::create(0x2000).expect("create VMO2 failed");
        let mut paddrs2 = [zx_paddr_t::default(); 2];
        let _pmt2 = bti
            .pin(zx::BtiOptions::PERM_READ, &vmo2, 0, 0x2000, &mut paddrs2[..])
            .expect("pin2 failed");

        let pinned = bti.get_pinned_vmo();

        let koid1 = vmo1.koid().unwrap();
        let koid2 = vmo2.koid().unwrap();

        let (v1, r1) = pinned.find_phys(0x1000).expect("lookup 0x1000 failed");
        assert_eq!(v1.koid().unwrap(), koid1);
        assert_eq!(r1, 0..0x2000);

        let (v1_2, r1_2) = pinned.find_phys(0x2000).expect("lookup 0x2000 failed");
        assert_eq!(v1_2.koid().unwrap(), koid1);
        assert_eq!(r1_2, 0x1000..0x2000);

        let (v1_3, r1_3) = pinned.find_phys(0x2fff).expect("lookup 0x2fff failed");
        assert_eq!(v1_3.koid().unwrap(), koid1);
        assert_eq!(r1_3, 0x1fff..0x2000);

        let (v2, r2) = pinned.find_phys(0x3000).expect("lookup 0x3000 failed");
        assert_eq!(v2.koid().unwrap(), koid2);
        assert_eq!(r2, 0..0x2000);

        let (v2_2, r2_2) = pinned.find_phys(0x4000).expect("lookup 0x4000 failed");
        assert_eq!(v2_2.koid().unwrap(), koid2);
        assert_eq!(r2_2, 0x1000..0x2000);

        assert!(pinned.find_phys(0x0fff).is_none());
        assert!(pinned.find_phys(0x5000).is_none());
    }

    #[fuchsia::test]
    fn test_read() {
        let bti = FakeBti::create().expect("create failed");

        {
            bti.set_paddrs(&[0x1000, 0x2000]); // Contiguous in physical space
            let vmo = zx::Vmo::create(0x2000).expect("create VMO failed");
            let data = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
            vmo.write(&data, 0xffc).expect("write failed");

            let mut paddrs = [0; 2];
            let _pmt = bti
                .pin(zx::BtiOptions::PERM_READ, &vmo, 0, 0x2000, &mut paddrs)
                .expect("pin failed");

            let pinned = bti.get_pinned_vmo();

            let val: u64 = pinned.read(0x1ffc).expect("read failed");
            assert_eq!(val, 0x8877665544332211);
        }

        // Test reading across non-contiguous physical ranges
        {
            let vmo = zx::Vmo::create(0x2000).expect("create VMO failed");
            let data = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
            vmo.write(&data, 0xffc).expect("write failed");
            vmo.write(&data, 0x1000).expect("write failed");

            // We must use distinct paddrs to avoid the fake-bti GetVmoPhysAddress bug.
            bti.set_paddrs(&[0x14000, 0x16000]);
            let mut paddrs2 = [0; 2];
            let _pmt2 = bti
                .pin(zx::BtiOptions::PERM_READ, &vmo, 0, 0x2000, &mut paddrs2)
                .expect("pin failed");
            assert_eq!(paddrs2, [0x14000, 0x16000]);

            let pinned2 = bti.get_pinned_vmo();
            // This should fail because of the gap at 0x15000
            assert!(pinned2.read::<u64>(0x14ffc).is_err());

            // But reading at the boundaries should work
            let val1: u32 = pinned2.read(0x14ffc).expect("read boundary 1 failed");
            assert_eq!(val1, 0x44332211);
            let val2: u32 = pinned2.read(0x16000).expect("read boundary 2 failed");
            assert_eq!(val2, 0x44332211);
        }
    }
}
