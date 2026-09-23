// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, Result};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_boot as fboot;
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, ChildRef, LocalComponentHandles, RealmBuilder, Route,
};
use fuchsia_driver_test::{
    DriverTestRealmBuilder, DriverTestRealmBuilder2, DriverTestRealmInstance,
    DriverTestRealmInstance2, Options2,
};
use futures::{FutureExt as _, StreamExt as _};
use zerocopy::IntoBytes;

async fn get_driver_info(
    service: &fdd::ManagerProxy,
    driver_filter: &[String],
) -> Result<Vec<fdf::DriverInfo>> {
    let (iterator, iterator_server) =
        fidl::endpoints::create_proxy::<fdd::DriverInfoIteratorMarker>();

    service
        .get_driver_info(driver_filter, iterator_server)
        .context("FIDL call to get driver info failed")?;

    let mut info_result = Vec::new();
    loop {
        let mut driver_info =
            iterator.get_next().await.context("FIDL call to get driver info failed")?;
        if driver_info.len() == 0 {
            break;
        }
        info_result.append(&mut driver_info)
    }
    Ok(info_result)
}

async fn serve_boot_items(handles: LocalComponentHandles) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::local(run_boot_items(stream)).detach();
    });
    fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;
    Ok(())
}

async fn run_boot_items(mut stream: fboot::ItemsRequestStream) {
    /// These constants are defined in
    /// zircon/system/ulib/ddk-platform-defs/include/lib/ddk/platform-defs.h
    const PDEV_VID_TEST: u32 = 0x11;
    const PDEV_PID_PBUS_TEST: u32 = 0x01;

    while let Some(request) = stream.next().await {
        match request.unwrap() {
            fboot::ItemsRequest::Get { type_: _, extra: _, responder } => {
                responder.send(None, 0).unwrap();
            }
            fboot::ItemsRequest::Get2 { type_, extra: _, responder } => {
                if type_ == zbi::Type::PlatformId as u32 {
                    let platform_id = zbi::PlatformId {
                        vid: PDEV_VID_TEST,
                        pid: PDEV_PID_PBUS_TEST,
                        board_name: [0; 32],
                    };
                    const PLATFORM_ID_SIZE: u32 = size_of::<zbi::PlatformId>() as u32;
                    let vmo = zx::Vmo::create(PLATFORM_ID_SIZE.into()).unwrap();
                    vmo.write(platform_id.as_bytes(), 0).unwrap();
                    let ret = vec![fboot::RetrievedItems {
                        payload: vmo,
                        length: PLATFORM_ID_SIZE,
                        extra: 0,
                    }];
                    responder.send(Ok(ret)).unwrap();
                } else {
                    responder.send(Err(zx::Status::NOT_SUPPORTED.into_raw())).unwrap();
                }
            }
            fboot::ItemsRequest::GetBootloaderFile { responder, filename: _ } => {
                responder.send(None).unwrap();
            }
        }
    }
}

#[fuchsia::test]
async fn test_smoke_test() -> Result<()> {
    let realm = RealmBuilder::new().await?;
    DriverTestRealmBuilder::driver_test_realm_setup(&realm).await?;

    let instance = realm.build().await?;

    instance.driver_test_realm_start(fdt::RealmArgs::default()).await?;

    // Connect to a protocol to ensure that it starts, then immediately exit.
    let _: ClientEnd<fdd::ManagerMarker> = instance.root.connect_to_protocol_at_exposed_dir()?;
    Ok(())
}

#[fuchsia::test]
async fn dtr_v2_test_smoke_test() -> Result<()> {
    let realm = RealmBuilder::new().await?;
    DriverTestRealmBuilder2::driver_test_realm_setup(
        &realm,
        Default::default(),
        fdt::RealmArgs::default(),
    )
    .await?;
    let instance = realm.build().await?;
    DriverTestRealmInstance2::wait_for_bootup(&instance).await?;

    // Connect to a protocol to ensure that it starts, then immediately exit.
    let dev = DriverTestRealmInstance2::driver_test_realm_connect_to_dev(&instance)?;
    device_watcher::recursive_wait(&dev, "sys/test/test2").await?;

    instance.destroy().await?;
    Ok(())
}

// Run DriverTestRealm with no arguments and see that the drivers in our package
// are loaded.
#[fuchsia::test]
async fn test_empty_args() -> Result<()> {
    let realm = RealmBuilder::new().await?;
    DriverTestRealmBuilder::driver_test_realm_setup(&realm).await?;

    let instance = realm.build().await?;

    instance.driver_test_realm_start(fdt::RealmArgs::default()).await?;

    let driver_dev = instance.root.connect_to_protocol_at_exposed_dir()?;

    let driver_urls = get_driver_info(&driver_dev, &[])
        .await?
        .into_iter()
        .map(|info| info.url)
        .collect::<Vec<Option<String>>>();
    assert!(
        driver_urls.contains(&Some("fuchsia-boot:///dtr#meta/test-parent-sys.cm".to_string())),
        "{:?}",
        driver_urls
    );
    assert!(
        driver_urls.contains(&Some("fuchsia-boot:///dtr#meta/test.cm".to_string())),
        "{:?}",
        driver_urls
    );

    Ok(())
}

#[fuchsia::test]
async fn dtr_v2_test_empty_args() -> Result<()> {
    let realm = RealmBuilder::new().await?;
    DriverTestRealmBuilder2::driver_test_realm_setup(
        &realm,
        Default::default(),
        fdt::RealmArgs::default(),
    )
    .await?;
    let instance = realm.build().await?;
    DriverTestRealmInstance2::wait_for_bootup(&instance).await?;

    let dev = DriverTestRealmInstance2::driver_test_realm_connect_to_dev(&instance)?;
    device_watcher::recursive_wait(&dev, "sys/test/test2").await?;

    let driver_dev = instance.root.connect_to_protocol_at_exposed_dir()?;

    let info = get_driver_info(&driver_dev, &[]).await?;
    assert!(
        info.iter()
            .any(|d| d.url == Some("fuchsia-boot:///dtr#meta/test-parent-sys.cm".to_string()))
    );
    assert!(info.iter().any(|d| d.url == Some("fuchsia-boot:///dtr#meta/test.cm".to_string())));

    instance.destroy().await?;
    Ok(())
}

// Manually open our /pkg directory and pass it to DriverTestRealm to see that it works.
#[fuchsia::test]
async fn test_pkg_dir() -> Result<()> {
    let realm = RealmBuilder::new().await?;
    DriverTestRealmBuilder::driver_test_realm_setup(&realm).await?;

    let instance = realm.build().await?;

    let (pkg, pkg_server) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
    let (boot, boot_server) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
    let pkg_flags = fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_EXECUTABLE;
    fuchsia_fs::directory::open_channel_in_namespace("/pkg", pkg_flags, boot_server).unwrap();
    // We send a bogus directory into pkg in order ensure we don't double index the same driver.
    fuchsia_fs::directory::open_channel_in_namespace("/pkg/bin", pkg_flags, pkg_server).unwrap();
    let args = fdt::RealmArgs { boot: Some(boot), pkg: Some(pkg), ..Default::default() };

    instance.driver_test_realm_start(args).await?;

    let driver_dev = instance.root.connect_to_protocol_at_exposed_dir()?;

    let driver_urls = get_driver_info(&driver_dev, &[])
        .await?
        .into_iter()
        .map(|info| info.url)
        .collect::<Vec<Option<String>>>();
    assert!(
        driver_urls.contains(&Some("fuchsia-boot:///dtr#meta/test-parent-sys.cm".to_string())),
        "{:?}",
        driver_urls
    );
    assert!(
        driver_urls.contains(&Some("fuchsia-boot:///dtr#meta/test.cm".to_string())),
        "{:?}",
        driver_urls
    );

    let dev = DriverTestRealmInstance::driver_test_realm_connect_to_dev(&instance)?;
    device_watcher::recursive_wait(&dev, "sys/test/test2").await?;

    Ok(())
}

#[fuchsia::test]
async fn dtr_v2_test_pkg_dir() -> Result<()> {
    let (pkg, pkg_server) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
    let (boot, boot_server) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
    let pkg_flags = fio::PERM_READABLE | fio::PERM_EXECUTABLE;
    fuchsia_fs::directory::open_channel_in_namespace("/pkg", pkg_flags, boot_server).unwrap();
    // We send a bogus directory into pkg in order ensure we don't double index the same driver.
    fuchsia_fs::directory::open_channel_in_namespace("/pkg/bin", pkg_flags, pkg_server).unwrap();
    let args = fdt::RealmArgs { boot: Some(boot), pkg: Some(pkg), ..Default::default() };

    let realm = RealmBuilder::new().await?;
    DriverTestRealmBuilder2::driver_test_realm_setup(&realm, Default::default(), args).await?;
    let instance = realm.build().await?;
    DriverTestRealmInstance2::wait_for_bootup(&instance).await?;

    let dev = DriverTestRealmInstance2::driver_test_realm_connect_to_dev(&instance)?;
    device_watcher::recursive_wait(&dev, "sys/test/test2").await?;

    let driver_dev = instance.root.connect_to_protocol_at_exposed_dir()?;

    let info = get_driver_info(&driver_dev, &[]).await?;
    assert!(
        info.iter()
            .any(|d| d.url == Some("fuchsia-boot:///dtr#meta/test-parent-sys.cm".to_string()))
    );
    assert!(info.iter().any(|d| d.url == Some("fuchsia-boot:///dtr#meta/test.cm".to_string())));

    instance.destroy().await?;
    Ok(())
}

#[fuchsia::test]
async fn test_root_driver() -> Result<()> {
    let realm = RealmBuilder::new().await?;
    DriverTestRealmBuilder::driver_test_realm_setup(&realm).await?;

    let instance = realm.build().await?;
    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///platform-bus#meta/platform-bus.cm".to_string()),
        ..Default::default()
    };

    instance.driver_test_realm_start(args).await?;

    let dev = DriverTestRealmInstance::driver_test_realm_connect_to_dev(&instance)?;
    device_watcher::recursive_wait(&dev, "sys/platform/pt").await?;

    Ok(())
}

#[fuchsia::test]
async fn dtr_v2_test_root_driver() -> Result<()> {
    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///platform-bus#meta/platform-bus.cm".to_string()),
        ..Default::default()
    };

    let realm = RealmBuilder::new().await?;
    DriverTestRealmBuilder2::driver_test_realm_setup(&realm, Default::default(), args).await?;
    let instance = realm.build().await?;
    DriverTestRealmInstance2::wait_for_bootup(&instance).await?;

    let dev = DriverTestRealmInstance2::driver_test_realm_connect_to_dev(&instance)?;
    device_watcher::recursive_wait(&dev, "sys/platform/pt").await?;

    instance.destroy().await?;
    Ok(())
}

#[fuchsia::test]
async fn test_tunnel_boot_items() -> Result<()> {
    let realm = RealmBuilder::new().await?;
    DriverTestRealmBuilder::driver_test_realm_setup(&realm).await?;

    let boot_items = realm
        .add_local_child("boot_items", move |h| serve_boot_items(h).boxed(), ChildOptions::new())
        .await?;
    let driver_test_realm: ChildRef = fuchsia_driver_test::COMPONENT_NAME.into();
    realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fboot::ItemsMarker>())
                .from(&boot_items)
                .to(&driver_test_realm),
        )
        .await?;

    realm.init_mutable_config_from_package(&driver_test_realm).await?;
    realm.set_config_value(&driver_test_realm, "tunnel_boot_items", true.into()).await?;

    let instance = realm.build().await?;
    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///platform-bus#meta/platform-bus.cm".to_string()),
        ..Default::default()
    };

    instance.driver_test_realm_start(args).await?;

    let dev = DriverTestRealmInstance::driver_test_realm_connect_to_dev(&instance)?;
    device_watcher::recursive_wait(&dev, "sys/platform/pt").await?;

    Ok(())
}

#[fuchsia::test]
async fn dtr_v2_test_tunnel_boot_items() -> Result<()> {
    let realm = RealmBuilder::new().await?;

    let boot_items = realm
        .add_local_child("boot_items", move |h| serve_boot_items(h).boxed(), ChildOptions::new())
        .await?;

    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///platform-bus#meta/platform-bus.cm".to_string()),
        ..Default::default()
    };

    DriverTestRealmBuilder2::driver_test_realm_setup(
        &realm,
        Options2::new().add_extra_realm_capability(
            Capability::protocol::<fboot::ItemsMarker>().into(),
            &boot_items,
        ),
        args,
    )
    .await?;
    let instance = realm.build().await?;
    DriverTestRealmInstance2::wait_for_bootup(&instance).await?;

    let dev = DriverTestRealmInstance2::driver_test_realm_connect_to_dev(&instance)?;
    device_watcher::recursive_wait(&dev, "sys/platform/pt").await?;

    instance.destroy().await?;
    Ok(())
}
