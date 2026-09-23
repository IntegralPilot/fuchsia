#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -euo pipefail

# Common runner logic for USB Zero Function desk test scripts.

# Global tracking variables for cleanup trap under set -u.
_ZERO_TEMP_YAML=""
_ZERO_TEMP_PARAMS_YAML=""
_ZERO_KEEP_DRIVER=false
_ZERO_DRIVER_LOADED=false

run_zero_desk_test() {
    local script_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
    local fuchsia_dir="${FUCHSIA_DIR:-}"
    if [[ -z "${fuchsia_dir}" ]]; then
        local root_rel="${script_dir}/../../../../../.."
        fuchsia_dir="$(cd "${root_rel}" >/dev/null 2>&1 && pwd)"
    fi
    local serial_socket="${FUCHSIA_SERIAL_SOCKET:-}"
    local keep_driver=false
    local dry_run=false
    local target_name=""
    local additional_args=()
    local temp_yaml=""
    local temp_params_yaml=""
    local driver_was_loaded=false

    local caller="${0}"
    local test_name="zero_function_test"
    local test_target=\
"//src/tests/end_to_end/usb/functional/zero_function:zero_function_test"
    local is_stress=false
    local stress_duration="${STRESS_DURATION:-30}"

    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --caller)
                caller="$2"
                shift 2
                ;;
            --dry-run)
                dry_run=true
                shift
                ;;
            --test-name)
                test_name="$2"
                shift 2
                ;;
            --test-target)
                test_target="$2"
                shift 2
                ;;
            --is-stress)
                is_stress=true
                shift
                ;;
            -s|--serial-socket)
                serial_socket="$2"
                shift 2
                ;;
            --serial-socket=*)
                serial_socket="${1#*=}"
                shift
                ;;
            -t|--target)
                target_name="$2"
                shift 2
                ;;
            --target=*)
                target_name="${1#*=}"
                shift
                ;;
            -d|--duration)
                stress_duration="$2"
                shift 2
                ;;
            --duration=*)
                stress_duration="${1#*=}"
                shift
                ;;
            --keep-driver)
                keep_driver=true
                shift
                ;;
            -h|--help)
                local exit_code=0
                cat <<USAGE_EOF
Usage: ${caller} --serial-socket <path> [options] [-- <extra fx test args>]

Mandatory Options:
  -s, --serial-socket <path>   Path to the device serial UNIX socket.

Optional Options:
  -t, --target <name>          Fuchsia target device name.
USAGE_EOF
                if [[ "${is_stress}" == "true" ]]; then
                    cat <<USAGE_EOF
  -d, --duration <sec>         Duration in seconds per routine (default: 30).
USAGE_EOF
                fi
                cat <<USAGE_EOF
  --keep-driver                Do not unload usbtest driver on completion.
  --dry-run                    Validate and print Mobly config without running.
  -h, --help                   Show this help message.

Examples:
  ${caller} --serial-socket /path/to/serial.sock
USAGE_EOF
                if [[ "${is_stress}" == "true" ]]; then
                    cat <<USAGE_EOF
  ${caller} -s /tmp/fuchsia_serial.sock -t fuchsia-8a0d-9418-80c8 -d 30
USAGE_EOF
                else
                    cat <<USAGE_EOF
  ${caller} -s /tmp/fuchsia_serial.sock -t fuchsia-8a0d-9418-80c8
USAGE_EOF
                fi
                exit "${exit_code}"
                ;;
            --)
                shift
                additional_args+=("$@")
                break
                ;;
            *)
                additional_args+=("$1")
                shift
                ;;
        esac
    done

    _ZERO_KEEP_DRIVER="${keep_driver}"

    cleanup() {
        local exit_code=$?
        trap - EXIT INT TERM
        local yaml_file="${temp_yaml:-${_ZERO_TEMP_YAML:-}}"
        if [[ -n "${yaml_file}" && -f "${yaml_file}" ]]; then
            rm -f "${yaml_file}"
        fi
        local params_file="${temp_params_yaml:-${_ZERO_TEMP_PARAMS_YAML:-}}"
        if [[ -n "${params_file}" && -f "${params_file}" ]]; then
            rm -f "${params_file}"
        fi
        if [[ "${keep_driver:-${_ZERO_KEEP_DRIVER:-false}}" == "true" ]]; then
            echo "[*] Keeping 'usbtest' driver loaded (--keep-driver)."
        elif [[ "${driver_was_loaded:-${_ZERO_DRIVER_LOADED:-false}}" == \
                "true" ]]; then
            echo "[*] Cleaning up host driver state..."
            echo "[*] Executing: sudo rmmod usbtest"
            if ! sudo rmmod usbtest 2>/dev/null; then
                echo "[!] Skipping usbtest module removal."
            else
                echo "[+] Successfully unloaded 'usbtest' kernel driver."
            fi
        fi
        exit "${exit_code}"
    }
    trap cleanup EXIT INT TERM

    if [[ -z "${serial_socket}" ]]; then
        echo "[!] ERROR: --serial-socket <path> is required." >&2
        return 1
    fi

    if [[ -c "${serial_socket}" || "${serial_socket}" == /dev/tty* ]]; then
        echo "[!] ERROR: Path '${serial_socket}' is a TTY char device." >&2
        echo "[!] A UNIX domain socket is required for serial comms." >&2
        echo "[!] To create a serial socket listener from this TTY, run:" >&2
        echo "[!]   socat UNIX-LISTEN:/tmp/fuchsia_serial.sock,fork,\\" >&2
        echo "[!]     reuseaddr FILE:${serial_socket},b115200,raw,echo=0 &" >&2
        echo "[!] Then pass '-s /tmp/fuchsia_serial.sock' to this script." >&2
        return 1
    fi

    if [[ ! -S "${serial_socket}" && ! -e "${serial_socket}" ]]; then
        echo "[!] WARNING: Serial socket '${serial_socket}' not found." >&2
    fi

    # Auto-detect default target if not passed
    if [[ -z "${target_name}" ]]; then
        target_name=$(ffx target default get 2>/dev/null || \
            ffx target list --format simple 2>/dev/null | \
            awk '{print $2}' | head -n 1 || echo "")
        if [[ -z "${target_name}" ]]; then
            echo "[!] ERROR: No Fuchsia target found via 'ffx target list'." >&2
            return 1
        fi
    fi

    # 1. Load host usbtest kernel module interactively if needed
    if [[ "${dry_run}" != "true" ]]; then
        if ! lsmod | grep -q "^usbtest " && \
           [[ ! -d "/sys/module/usbtest" ]]; then
            echo "[*] Host kernel driver 'usbtest' is not loaded."
            echo "[*] Executing: sudo modprobe usbtest vendor=0x18d1 \\"
            echo "[*]   product=0xa022 alt=0"
            if sudo modprobe usbtest vendor=0x18d1 product=0xa022 alt=0; then
                driver_was_loaded=true
                _ZERO_DRIVER_LOADED=true
                echo "[+] Successfully loaded 'usbtest' kernel driver."
            else
                echo "[!] ERROR: Failed to load 'usbtest' module via sudo." >&2
                return 1
            fi
        else
            echo "[+] Host kernel driver 'usbtest' is already loaded."
        fi
    fi

    # Cleanup handler for rmmod & temp files
    temp_yaml=$(mktemp "/tmp/${test_name}_testbed_XXXXXX.yaml")
    _ZERO_TEMP_YAML="${temp_yaml}"

    # 2. Get target serial number and IP for Mobly config
    local target_serial
    local target_addr
    target_serial=$(ffx -t "${target_name}" --machine json target show \
        2>/dev/null | grep -o '"serial_number":"[^"]*"' | \
        cut -d'"' -f4 || echo "")
    target_addr=$(ffx -t "${target_name}" --machine json target show \
        2>/dev/null | grep -o '"host":"[^"]*"' | cut -d'"' -f4 || echo "")

    if [[ -z "${target_serial}" || -z "${target_addr}" ]]; then
        local list_json
        if list_json=$(ffx target list --format json 2>/dev/null); then
            local json_serial=""
            local json_addr=""
            read -r json_serial json_addr < <(echo "${list_json}" | python3 -c '
import json, sys
try:
    data = json.load(sys.stdin)
    target = sys.argv[1] if len(sys.argv) > 1 else ""
    for item in data:
        if (target and item.get("nodename") == target) or \
           (not target and item.get("is_default")):
            serial = item.get("serial", "")
            addrs = item.get("addresses", [])
            ip = addrs[0].get("ip", "") if addrs else ""
            print(f"{serial} {ip}")
            break
except Exception:
    pass
' "${target_name}" || echo "") || true
            target_serial="${target_serial:-${json_serial:-}}"
            target_addr="${target_addr:-${json_addr:-}}"
        fi
    fi

    if [[ -z "${target_serial}" || -z "${target_addr}" ]]; then
        local target_clean
        target_clean=$(echo "${target_name}" | tr -d '-' | sed 's/^fuchsia//')
        local zx_ifaces
        zx_ifaces=$(ip -o link show 2>/dev/null | \
            awk -F': ' '$2 ~ /^zx-/ {print $2}')
        for iface in ${zx_ifaces}; do
            if [[ -n "${target_clean}" && \
                  "${iface}" != *"${target_clean}"* ]]; then
                continue
            fi
            local neigh_ip
            neigh_ip=$(ip -6 neigh show dev "${iface}" 2>/dev/null | \
                awk '$4 == "REACHABLE" {print $1; exit}')
            if [[ -n "${neigh_ip}" ]]; then
                local show_out
                show_out=$(ffx -t "${neigh_ip}%${iface}" --machine json \
                    target show 2>/dev/null || true)
                if [[ -n "${show_out}" ]]; then
                    target_serial="${target_serial:-$(echo "${show_out}" | \
                        grep -o '"serial_number":"[^"]*"' | \
                        cut -d'"' -f4 || echo "")}"
                    target_addr="${target_addr:-${neigh_ip}%${iface}}"
                    break
                fi
            fi
        done
    fi

    # 3. Generate Mobly YAML configuration
    cat <<TESTBED_EOF > "${temp_yaml}"
MoblyParams:
  LogPath: /tmp/${USER:-fuchsia}_zero_function_mobly_logs
TESTBED_EOF

    if [[ "${is_stress}" == "true" ]]; then
        cat <<STRESS_PARAM_EOF >> "${temp_yaml}"
TestParams:
  stress_duration_sec: ${stress_duration}
STRESS_PARAM_EOF
    fi

    cat <<TESTBED_CONTROLLER_EOF >> "${temp_yaml}"
TestBeds:
- Name: GeneratedLocalTestbed
TESTBED_CONTROLLER_EOF

    if [[ "${is_stress}" == "true" ]]; then
        cat <<STRESS_PARAM_TESTBED_EOF >> "${temp_yaml}"
  TestParams:
    stress_duration_sec: ${stress_duration}
STRESS_PARAM_TESTBED_EOF
    fi

    cat <<CONTROLLERS_EOF >> "${temp_yaml}"
  Controllers:
    FuchsiaDevice:
    - name: "${target_name}"
      device_serial: "${target_serial}"
      serial_socket: "${serial_socket}"
      honeydew_config:
        transports:
          ffx:
            enable_usb: true
CONTROLLERS_EOF

    if [[ -n "${target_addr}" ]]; then
        cat <<IP_PORT_EOF >> "${temp_yaml}"
      device_ip_port: "[${target_addr}]:22"
IP_PORT_EOF
    fi

    echo "[*] Target device: ${target_name} (Serial: ${target_serial})"
    echo "[*] Generated Mobly testbed config with serial socket:"
    echo "[*]   ${serial_socket}"

    # 4. Run Mobly test via fx test
    local -a test_cmd=(
        "${fuchsia_dir}/.jiri_root/bin/fx"
    )
    local fx_target="${target_addr:-${target_name}}"
    if [[ -n "${fx_target}" ]]; then
        test_cmd+=("-t" "${fx_target}")
    fi
    test_cmd+=(
        test
        -o
        --e2e
        --no-allow-temporary-emulator
        "${test_target}"
        --
        --config-yaml-path "${temp_yaml}"
    )

    if [[ "${is_stress}" == "true" ]]; then
        temp_params_yaml=$(mktemp "/tmp/${test_name}_params_XXXXXX.yaml")
        _ZERO_TEMP_PARAMS_YAML="${temp_params_yaml}"
        cat <<PARAMS_EOF > "${temp_params_yaml}"
stress_duration_sec: ${stress_duration}
PARAMS_EOF
        echo "[*] Configured stress duration per routine: ${stress_duration}s"
        test_cmd+=(--params-yaml-path "${temp_params_yaml}")
    fi

    test_cmd+=("${additional_args[@]}")

    if [[ "${dry_run}" == "true" ]]; then
        echo "[*] [DRY RUN] Generated Mobly YAML (${temp_yaml}):"
        cat "${temp_yaml}"
        if [[ "${is_stress}" == "true" && -n "${temp_params_yaml:-}" ]]; then
            echo "[*] [DRY RUN] Generated Params YAML (${temp_params_yaml}):"
            cat "${temp_params_yaml}"
        fi
        echo "[*] [DRY RUN] Would execute command:"
        echo "    ${test_cmd[*]}"
        return 0
    fi

    echo "[*] Executing ${test_name}..."
    "${test_cmd[@]}"
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    run_zero_desk_test "$@"
fi
