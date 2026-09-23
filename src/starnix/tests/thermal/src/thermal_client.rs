// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use linux_uapi::{THERMAL_GENL_EVENT_GROUP_NAME, THERMAL_GENL_SAMPLING_GROUP_NAME};
use netlink_packet_core::{NLM_F_REQUEST, NetlinkMessage, NetlinkPayload};
use netlink_packet_generic::GenlMessage;
use netlink_packet_generic::ctrl::nlas::{GenlCtrlAttrs, McastGrpAttrs};
use netlink_packet_generic::ctrl::{GenlCtrl, GenlCtrlCmd};
use netlink_packet_generic::message::EmptyDeserializeOptions;
use nix::sys::socket;
use std::collections::{HashMap, HashSet};
use std::os::fd::{AsFd, AsRawFd};
use std::time::{Duration, Instant};
use thermal_netlink::{GenlThermalCmd, GenlThermalPayload, ThermalAttr, celsius_to_millicelsius};

pub const EXPECTED_TEMP_C: f32 = 25.0;
const FREQUENCIES_HZ: [&'static str; 2] = ["1128000 1256000 1512000 2024000", "512000 1024000"];
const MAX_FREQUENCIES_HZ: [&'static str; 2] = ["2024000", "1024000"];

fn main() {
    println!("started");
    check_thermal_zone_is_available();
    check_emul_temp();

    check_cpu_cooling_device_is_available();
    check_cpu_cooling_device();
    check_cpufreq();

    check_nlctrl_is_available();

    check_cooling_fcc_is_available();
    check_cooling_fcc();

    let thermal_mcast_groups = check_thermal_is_available();
    let sampling_group_id =
        thermal_mcast_groups.get(THERMAL_GENL_SAMPLING_GROUP_NAME.to_str().unwrap()).unwrap();
    check_thermal_sampling_returns_samples(*sampling_group_id);
    println!("done");
}

fn check_thermal_zone_is_available() {
    let sensor_name = std::fs::read("/sys/class/thermal/thermal_zone0/type").unwrap();
    assert_eq!("fake-trippoint\n", str::from_utf8(&sensor_name).unwrap());

    // Due to races between DriverTestRealm and the test environment the
    // expected value may not immediately be set. Loop until we get a match.
    let now = Instant::now();
    loop {
        let temp_c = std::fs::read("/sys/class/thermal/thermal_zone0/temp").unwrap();
        if str::from_utf8(&temp_c).unwrap()
            == &format!("{}\n", celsius_to_millicelsius(EXPECTED_TEMP_C) as u32)
        {
            break;
        }
        if now.elapsed() > Duration::from_secs(5) {
            println!("Temperature reading taking longer than 5 seconds...");
        }
    }
}

fn check_emul_temp() {
    {
        let real_temp_str = std::fs::read("/sys/class/thermal/thermal_zone0/temp").unwrap();
        let expected_real_temp = celsius_to_millicelsius(EXPECTED_TEMP_C) as u32;
        assert_eq!(&format!("{}\n", expected_real_temp), str::from_utf8(&real_temp_str).unwrap());
    }
    {
        let expected_temp = "100000\n";
        std::fs::write("/sys/class/thermal/thermal_zone0/emul_temp", expected_temp).unwrap();
        let fake_temp_str = std::fs::read("/sys/class/thermal/thermal_zone0/temp").unwrap();
        assert_eq!(expected_temp, str::from_utf8(&fake_temp_str).unwrap());
    }
    {
        let expected_temp = "50000\n";
        std::fs::write("/sys/class/thermal/thermal_zone0/emul_temp", expected_temp).unwrap();
        let fake_temp_str = std::fs::read("/sys/class/thermal/thermal_zone0/temp").unwrap();
        assert_eq!(expected_temp, str::from_utf8(&fake_temp_str).unwrap());
    }

    // Reset emul_temp.
    std::fs::write("/sys/class/thermal/thermal_zone0/emul_temp", "0").unwrap();
    let real_temp_str = std::fs::read("/sys/class/thermal/thermal_zone0/temp").unwrap();
    let expected_real_temp = celsius_to_millicelsius(EXPECTED_TEMP_C) as u32;
    assert_eq!(&format!("{}\n", expected_real_temp), str::from_utf8(&real_temp_str).unwrap());
}

fn check_cpu_cooling_device_is_available() {
    {
        let sensor_name = std::fs::read("/sys/class/thermal/cooling_device0/type").unwrap();
        assert_eq!("test-cluster0\n", str::from_utf8(&sensor_name).unwrap());
        let cur_state = std::fs::read("/sys/class/thermal/cooling_device0/cur_state").unwrap();
        assert_eq!("0\n", str::from_utf8(&cur_state).unwrap());
        let max_state = std::fs::read("/sys/class/thermal/cooling_device0/max_state").unwrap();
        assert_eq!("3\n", str::from_utf8(&max_state).unwrap());
    }

    {
        let sensor_name = std::fs::read("/sys/class/thermal/cooling_device1/type").unwrap();
        assert_eq!("test-cluster1\n", str::from_utf8(&sensor_name).unwrap());
        let cur_state = std::fs::read("/sys/class/thermal/cooling_device1/cur_state").unwrap();
        assert_eq!("0\n", str::from_utf8(&cur_state).unwrap());
        let max_state = std::fs::read("/sys/class/thermal/cooling_device1/max_state").unwrap();
        assert_eq!("1\n", str::from_utf8(&max_state).unwrap());
    }
}

fn check_cpu_cooling_device() {
    std::fs::write("/sys/class/thermal/cooling_device0/cur_state", "1").unwrap();
    let new_cur_state = std::fs::read("/sys/class/thermal/cooling_device0/cur_state").unwrap();
    assert_eq!("1\n", str::from_utf8(&new_cur_state).unwrap());
}

fn check_cpufreq() {
    assert_eq!(
        "0-5\n",
        str::from_utf8(&std::fs::read("/sys/devices/system/cpu/possible").unwrap()).unwrap()
    );
    check_cpufreq_dir(0, 0);
    check_cpufreq_dir(1, 0);
    check_cpufreq_dir(2, 1);
    check_cpufreq_dir(3, 1);
    check_cpufreq_dir(4, 1);
    check_cpufreq_dir(5, 1);

    check_cpufreq_policy_dir(0, 0, "0 1");
    check_cpufreq_policy_dir(2, 1, "2 3 4 5");
}

fn check_cpufreq_policy_dir(policy_id: u64, cluster_id: u64, expected_related_cpus: &str) {
    assert!(std::fs::exists(format!("/sys/devices/system/cpu/cpufreq/policy{policy_id}")).unwrap());

    let max_frequency_str = MAX_FREQUENCIES_HZ[cluster_id as usize];
    assert_eq!(
        &format!("{max_frequency_str}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpufreq/policy{policy_id}/cpuinfo_max_freq"
            ))
            .unwrap()
        )
        .unwrap()
    );

    let frequencies_str = FREQUENCIES_HZ[cluster_id as usize];
    assert_eq!(
        &format!("{frequencies_str}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpufreq/policy{policy_id}/scaling_available_frequencies"
            ))
            .unwrap()
        )
        .unwrap()
    );

    assert_eq!(
        &format!("{expected_related_cpus}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpufreq/policy{policy_id}/related_cpus"
            ))
            .unwrap()
        )
        .unwrap()
    );

    let cur_frequency_str = MAX_FREQUENCIES_HZ[cluster_id as usize];
    assert_eq!(
        &format!("{cur_frequency_str}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpufreq/policy{policy_id}/scaling_cur_freq"
            ))
            .unwrap()
        )
        .unwrap()
    );
}

fn check_cpufreq_dir(core_id: u64, cluster_id: u64) {
    assert!(std::fs::exists(format!("/sys/devices/system/cpu/cpu{core_id}")).unwrap());
    assert!(std::fs::exists(format!("/sys/devices/system/cpu/cpu{core_id}/cpufreq")).unwrap());

    let max_frequency_str = MAX_FREQUENCIES_HZ[cluster_id as usize];
    assert_eq!(
        &format!("{max_frequency_str}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpu{core_id}/cpufreq/cpuinfo_max_freq"
            ))
            .unwrap()
        )
        .unwrap()
    );

    let frequencies_str = FREQUENCIES_HZ[cluster_id as usize];
    assert_eq!(
        &format!("{frequencies_str}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpu{core_id}/cpufreq/scaling_available_frequencies"
            ))
            .unwrap()
        )
        .unwrap()
    );

    assert_eq!(
        &format!("{max_frequency_str}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpu{core_id}/cpufreq/scaling_cur_freq"
            ))
            .unwrap()
        )
        .unwrap()
    );

    assert!(std::fs::exists(format!("/sys/devices/system/cpu/cpu{core_id}/topology")).unwrap());
    assert_eq!(
        &format!("{cluster_id}\n"),
        str::from_utf8(
            &std::fs::read(format!("/sys/devices/system/cpu/cpu{core_id}/topology/cluster_id"))
                .unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        &format!("{cluster_id}\n"),
        str::from_utf8(
            &std::fs::read(format!(
                "/sys/devices/system/cpu/cpu{core_id}/topology/physical_package_id"
            ))
            .unwrap()
        )
        .unwrap()
    );
}

fn check_nlctrl_is_available() {
    let nl_socket = socket::socket(
        socket::AddressFamily::Netlink,
        socket::SockType::Datagram,
        socket::SockFlag::SOCK_CLOEXEC,
        socket::SockProtocol::NetlinkGeneric,
    )
    .unwrap();
    socket::bind(nl_socket.as_raw_fd(), &socket::NetlinkAddr::new(0, 0)).unwrap();
    socket::connect(nl_socket.as_raw_fd(), &socket::NetlinkAddr::new(0, 0)).unwrap();

    let mut genlmsg = GenlMessage::from_payload(GenlCtrl {
        cmd: GenlCtrlCmd::GetFamily,
        nlas: vec![GenlCtrlAttrs::FamilyName("nlctrl".to_owned())],
    });
    genlmsg.finalize();
    let mut nlmsg = NetlinkMessage::from(genlmsg);
    nlmsg.header.flags = NLM_F_REQUEST;
    nlmsg.finalize();

    let mut txbuf = vec![0u8; nlmsg.buffer_len()];
    nlmsg.serialize(&mut txbuf);

    socket::send(nl_socket.as_raw_fd(), &txbuf, socket::MsgFlags::empty()).unwrap();

    let mut rxbuf = vec![0u8; 1024];
    socket::recvfrom::<socket::NetlinkAddr>(nl_socket.as_raw_fd(), &mut rxbuf).unwrap();
    let rx_packet =
        <NetlinkMessage<GenlMessage<GenlCtrl>>>::deserialize(&rxbuf, EmptyDeserializeOptions)
            .unwrap();

    if let NetlinkPayload::InnerMessage(genlmsg) = rx_packet.payload {
        if GenlCtrlCmd::NewFamily == genlmsg.payload.cmd {
            let family_id = genlmsg
                .payload
                .nlas
                .iter()
                .find_map(
                    |nla| {
                        if let GenlCtrlAttrs::FamilyId(id) = nla { Some(*id) } else { None }
                    },
                )
                .expect("Cannot find FamilyId attribute");
            // nlctrl's family must be 16.
            assert_eq!(16, family_id);
        } else {
            panic!("Invalid payload type: {:?}", genlmsg.payload.cmd);
        }
    } else {
        panic!("Failed to get family ID");
    }
}

fn check_cooling_fcc_is_available() {
    let sensor_name = std::fs::read("/sys/class/thermal/cooling_device2/type").unwrap();
    assert_eq!("fcc\n", str::from_utf8(&sensor_name).unwrap());
    let cur_state = std::fs::read("/sys/class/thermal/cooling_device2/cur_state").unwrap();
    assert_eq!("0\n", str::from_utf8(&cur_state).unwrap());
    let max_state = std::fs::read("/sys/class/thermal/cooling_device2/max_state").unwrap();
    assert_eq!("8\n", str::from_utf8(&max_state).unwrap());
}

fn check_cooling_fcc() {
    std::fs::write("/sys/class/thermal/cooling_device2/cur_state", "8").unwrap();
    let new_cur_state = std::fs::read("/sys/class/thermal/cooling_device2/cur_state").unwrap();
    assert_eq!("8\n", str::from_utf8(&new_cur_state).unwrap());

    // Writes greater than max_state wrap to 0.
    std::fs::write("/sys/class/thermal/cooling_device2/cur_state", "9").unwrap();
    let new_cur_state = std::fs::read("/sys/class/thermal/cooling_device2/cur_state").unwrap();
    assert_eq!("0\n", str::from_utf8(&new_cur_state).unwrap());
}

#[derive(Clone)]
struct NetlinkAddMembership;

impl socket::SetSockOpt for NetlinkAddMembership {
    type Val = u32;

    fn set<F: AsFd>(&self, fd: &F, val: &Self::Val) -> nix::Result<()> {
        unsafe {
            let res = libc::setsockopt(
                fd.as_fd().as_raw_fd(),
                libc::SOL_NETLINK,
                libc::NETLINK_ADD_MEMBERSHIP,
                <*const _>::cast(val),
                std::mem::size_of_val(val) as libc::socklen_t,
            );
            nix::Error::result(res).map(drop)
        }
    }
}

fn check_thermal_is_available() -> HashMap<String, u32> {
    let nl_socket = socket::socket(
        socket::AddressFamily::Netlink,
        socket::SockType::Datagram,
        socket::SockFlag::SOCK_CLOEXEC,
        socket::SockProtocol::NetlinkGeneric,
    )
    .unwrap();
    socket::bind(nl_socket.as_raw_fd(), &socket::NetlinkAddr::new(0, 0)).unwrap();
    socket::connect(nl_socket.as_raw_fd(), &socket::NetlinkAddr::new(0, 0)).unwrap();

    let mut genlmsg = GenlMessage::from_payload(GenlCtrl {
        cmd: GenlCtrlCmd::GetFamily,
        nlas: vec![GenlCtrlAttrs::FamilyName("thermal".to_owned())],
    });
    genlmsg.finalize();
    let mut nlmsg = NetlinkMessage::from(genlmsg);
    nlmsg.header.flags = NLM_F_REQUEST;
    nlmsg.finalize();

    let mut txbuf = vec![0u8; nlmsg.buffer_len()];
    nlmsg.serialize(&mut txbuf);

    socket::send(nl_socket.as_raw_fd(), &txbuf, socket::MsgFlags::empty()).unwrap();

    let mut rxbuf = vec![0u8; 1024];
    socket::recvfrom::<socket::NetlinkAddr>(nl_socket.as_raw_fd(), &mut rxbuf).unwrap();
    let rx_packet =
        <NetlinkMessage<GenlMessage<GenlCtrl>>>::deserialize(&rxbuf, EmptyDeserializeOptions)
            .unwrap();

    let genlmsg = assert_matches!(rx_packet.payload, NetlinkPayload::InnerMessage(g) => g);
    assert_eq!(genlmsg.payload.cmd, GenlCtrlCmd::NewFamily);

    let family_id = genlmsg
        .payload
        .nlas
        .iter()
        .find_map(|nla| if let GenlCtrlAttrs::FamilyId(id) = nla { Some(*id) } else { None })
        .expect("Cannot find FamilyId attribute");
    assert!(family_id > 16);

    let groups = genlmsg
        .payload
        .nlas
        .iter()
        .find_map(|nla| {
            if let GenlCtrlAttrs::McastGroups(groups) = nla {
                let mut group_map: HashMap<String, u32> = HashMap::new();
                for group in groups {
                    let name = assert_matches!(&group[0], McastGrpAttrs::Name(name) => name);
                    let id = assert_matches!(&group[1], McastGrpAttrs::Id(id) => id);
                    group_map.insert(name.clone(), *id);
                }
                Some(group_map)
            } else {
                None
            }
        })
        .expect("Cannot find FamilyId attribute");

    let mut expected_groups = HashSet::new();
    expected_groups.insert(THERMAL_GENL_SAMPLING_GROUP_NAME.to_str().unwrap().to_string());
    expected_groups.insert(THERMAL_GENL_EVENT_GROUP_NAME.to_str().unwrap().to_string());

    assert_eq!(expected_groups.len(), groups.len());
    assert_eq!(expected_groups, groups.keys().map(|s| s.to_string()).collect::<HashSet<String>>());
    return groups;
}

fn check_thermal_sampling_returns_samples(sampling_group_id: u32) {
    let nl_socket = socket::socket(
        socket::AddressFamily::Netlink,
        socket::SockType::Datagram,
        socket::SockFlag::SOCK_CLOEXEC,
        socket::SockProtocol::NetlinkGeneric,
    )
    .unwrap();
    socket::bind(nl_socket.as_raw_fd(), &socket::NetlinkAddr::new(0, 0)).unwrap();
    socket::connect(nl_socket.as_raw_fd(), &socket::NetlinkAddr::new(0, 0)).unwrap();
    socket::setsockopt(&nl_socket, NetlinkAddMembership, &sampling_group_id).unwrap();

    let now = std::time::Instant::now();
    for _ in 0..3 {
        let mut rxbuf = vec![0u8; 256];
        let (recv_size, _addr) =
            socket::recvfrom::<socket::NetlinkAddr>(nl_socket.as_raw_fd(), &mut rxbuf).unwrap();
        assert!(recv_size > 0);

        println!(
            "Received {} bytes after {} seconds: {:?}",
            recv_size,
            now.elapsed().as_secs(),
            &rxbuf[..recv_size]
        );

        let rx_packet = <NetlinkMessage<GenlMessage<GenlThermalPayload>>>::deserialize(
            &rxbuf,
            EmptyDeserializeOptions,
        )
        .unwrap();
        let genlmsg = assert_matches!(rx_packet.payload, NetlinkPayload::InnerMessage(m) => m);
        assert_eq!(GenlThermalCmd::ThermalGenlSamplingTemp, genlmsg.payload.cmd);

        assert_eq!(2, genlmsg.payload.nlas.len());
        let id = assert_matches!(genlmsg.payload.nlas[0], ThermalAttr::ThermalZoneId(id) => id);
        let temp =
            assert_matches!(genlmsg.payload.nlas[1], ThermalAttr::ThermalZoneTemp(temp) => temp);

        // ID should match the thermal zone number.
        assert_eq!(0u32, id);
        assert_eq!(celsius_to_millicelsius(EXPECTED_TEMP_C) as u32, temp);
    }

    // Should take less than 10 seconds to get 3 samples.
    // This assumes the thermal netlink server serves samples every 2 seconds
    // plus some buffer time for test variance.
    assert!(now.elapsed().as_secs() < 10);
}
