// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use crate::args::{Args, ShowCommand, SubCommand};
use anyhow::{Result, format_err};
use fidl_fuchsia_io as fio;
use fuchsia_pkg::MetaContents;
use std::collections::HashSet;
use std::fs::File;
use std::io::{Cursor, Read};

pub fn bootpkg(boot_dir: File, args: Args) -> Result<()> {
    match args.command {
        SubCommand::List(_) => list(&boot_dir),
        SubCommand::Show(ShowCommand { package_name }) => show(&boot_dir, &package_name),
    }
}

fn read_file(boot_dir: &File, path: &str) -> Result<Vec<u8>> {
    let mut file = fdio::open_fd_at(boot_dir, path, fio::PERM_READABLE)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(buffer)
}

fn get_package_list(boot_dir: &File) -> Result<MetaContents> {
    let contents = read_file(boot_dir, "data/bootfs_packages")?;
    Ok(MetaContents::deserialize(&contents[..])?)
}

fn list(boot_dir: &File) -> Result<()> {
    let package_list = get_package_list(boot_dir)?;
    for name in package_list.keys() {
        println!("{name}");
    }

    Ok(())
}

fn show(boot_dir: &File, package_name: &str) -> Result<()> {
    let package_list = get_package_list(boot_dir)?;
    let Some(merkle) = package_list.get(package_name) else {
        return Err(format_err!("package '{package_name}' not found in package list"));
    };
    let meta_far = read_file(boot_dir, &format!("blob/{merkle}"))?;

    let mut reader = fuchsia_archive::Reader::new(Cursor::new(meta_far))?;
    let reader_list = reader.list();
    let mut meta_files = HashSet::with_capacity(reader_list.len());
    for entry in reader_list {
        let path = String::from_utf8(entry.path().to_vec())?;
        if path.starts_with("meta/") {
            for (i, _) in path.match_indices('/').skip(1) {
                if meta_files.contains(&path[..i]) {
                    return Err(format_err!("Colliding entries in meta archive"));
                }
            }
            meta_files.insert(path);
        }
    }

    let meta_contents_bytes = reader.read_file(b"meta/contents")?;
    let non_meta_files = MetaContents::deserialize(&meta_contents_bytes[..])?.into_contents();

    let mut files = meta_files
        .iter()
        .map(|s| s.as_str())
        .chain(non_meta_files.keys().filter(|name| *name != "/0"))
        .collect::<Vec<_>>();
    files.sort();

    for name in files {
        println!("{name}");
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::endpoints::Proxy as _;
    use fuchsia_async as fasync;
    use fuchsia_merkle::Hash;
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::str::FromStr;
    use vfs::file::vmo::read_only;
    use vfs::pseudo_directory;

    #[fasync::run_singlethreaded(test)]
    async fn list_test() {
        // prep boot directory.
        let contents = MetaContents::from([
            (
                "foo",
                Hash::from_str("b21b34f8370687249a9cd9d4b306dee4c81f1f854f84de4626dc00c000c902fe")
                    .unwrap(),
            ),
            (
                "bar",
                Hash::from_str("d0ff2aa87c938862d56fff76c9fe362240d2d51699506753e5840e05d41a3bf2")
                    .unwrap(),
            ),
            (
                "baz",
                Hash::from_str("d0ff2aa87c938862d56fff76c9fe362240d2d51699506753e5840e05d41a3bf2")
                    .unwrap(),
            ),
        ]);
        let mut data = Vec::new();
        contents.serialize(&mut data).unwrap();

        let boot_dir = pseudo_directory! {
            "data" => pseudo_directory! {
                "bootfs_packages" => read_only(data),
            },
        };
        let dir_client =
            vfs::directory::serve_read_only(boot_dir, vfs::execution_scope::ExecutionScope::new());
        fasync::unblock(move || {
            let channel = dir_client.into_channel().unwrap().into_zx_channel();
            let client: File = fdio::create_fd(channel.into()).unwrap().into();
            list(&client).unwrap();
        })
        .await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn show_test() {
        // prep boot directory.
        let contents = MetaContents::from([
            (
                "foo",
                Hash::from_str("b21b34f8370687249a9cd9d4b306dee4c81f1f854f84de4626dc00c000c902fe")
                    .unwrap(),
            ),
            (
                "bar",
                Hash::from_str("d0ff2aa87c938862d56fff76c9fe362240d2d51699506753e5840e05d41a3bf2")
                    .unwrap(),
            ),
            (
                "baz",
                Hash::from_str("d0ff2aa87c938862d56fff76c9fe362240d2d51699506753e5840e05d41a3bf2")
                    .unwrap(),
            ),
        ]);
        let mut data = Vec::new();
        contents.serialize(&mut data).unwrap();

        let meta_contents = data.clone();
        let mut path_content_map: BTreeMap<&str, (u64, Box<dyn Read>)> = BTreeMap::new();
        path_content_map
            .insert("meta/contents", (meta_contents.len() as u64, Box::new(&meta_contents[..])));
        let mut far_contents = Vec::new();
        fuchsia_archive::write(&mut far_contents, path_content_map).unwrap();

        let boot_dir = pseudo_directory! {
            "data" => pseudo_directory! {
                "bootfs_packages" => read_only(data),
            },
            "blob" => pseudo_directory! {
                "b21b34f8370687249a9cd9d4b306dee4c81f1f854f84de4626dc00c000c902fe" => read_only(far_contents),
            }
        };
        let dir_client =
            vfs::directory::serve_read_only(boot_dir, vfs::execution_scope::ExecutionScope::new());
        fasync::unblock(move || {
            let channel = dir_client.into_channel().unwrap().into_zx_channel();
            let client: File = fdio::create_fd(channel.into()).unwrap().into();
            show(&client, "foo").unwrap();
        })
        .await;
    }
}
