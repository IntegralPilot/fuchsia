// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const PAGE_SIZE: usize = 0x1000;

macro_rules! debug_assert_ok {
    ($result:expr) => {
        debug_assert!($result.is_ok(), "{:#?}", $result);
    };
}
pub(crate) use debug_assert_ok;

pub trait LogError {
    #[track_caller]
    fn log_err(self, str: impl std::fmt::Display) -> Self;
}

impl<T, R> LogError for Result<T, R>
where
    R: std::fmt::Display,
{
    #[track_caller]
    fn log_err(self, str: impl std::fmt::Display) -> Self {
        self.inspect_err(|e| log::error!("{}: {}", str, e))
    }
}

pub async fn load_file_to_vmo(
    context: &fdf_component::DriverContext,
    path: &str,
) -> Result<zx::Vmo, zx::Status> {
    use fidl_fuchsia_io as fio;
    use fuchsia_component::directory::Directory;

    let (file_proxy, server_end) = fidl::endpoints::create_proxy::<fio::FileMarker>();
    context
        .incoming
        .open(
            path,
            fio::Flags::PERM_READ_BYTES | fio::Flags::PROTOCOL_FILE,
            server_end.into_channel(),
        )
        .log_err("Failed to open file")
        .map_err(|_| zx::Status::INTERNAL)?;

    let vmo = file_proxy
        .get_backing_memory(fio::VmoFlags::READ)
        .await
        .log_err("Failed to call 'get_backing_memory'")
        .map_err(|_| zx::Status::INTERNAL)?
        .log_err("Failed 'get_backing_memory'")
        .map_err(|_| zx::Status::INTERNAL)?;
    Ok(vmo)
}

pub fn do_until<I, F, T>(range: I, f: F) -> Option<T>
where
    I: IntoIterator,
    F: FnMut(I::Item) -> Option<T>,
{
    range.into_iter().find_map(f)
}

pub fn lower_u32(data: u64) -> u32 {
    data as u32
}

pub fn upper_u32(data: u64) -> u32 {
    (data >> 32) as u32
}

pub fn upper_u32_to_u64(data: u32) -> u64 {
    (data as u64) << 32
}

pub fn assert_aligned<T>(addr: u64) {
    let alignment = std::mem::align_of::<T>() as u64;
    assert!(
        addr % alignment == 0,
        "Address 0x{:x} is not aligned to {} bytes for type '{}'",
        addr,
        alignment,
        std::any::type_name::<T>()
    );
}

pub fn round_up_to_page_size(value: usize) -> usize {
    (value + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}
