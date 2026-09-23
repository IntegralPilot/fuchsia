# A puppet manifest to configure Gnome NetworkManager for Zircon CDC devices.
#
# This script installs the necessary udev and NetworkManager configuration for
# Zircon CDC devices. The following files will added (or modified if contents
# differ):
#
#  * /etc/udev/rules.d/70-zircon-ethernet.rules
#  * /etc/NetworkManager/system-connections/fuchsia_cdc.nmconnection
#  * /etc/NetworkManager/conf.d/fuchsia.conf
#
# No Fuchsia tree is required. This manifest needs to applied as root and
# assumes NetworkManager is already installed (i.e. fully configuring an
# uninstalled NetworkManager is beyond the scope of this manifest).
#
# Before deployment, it is highly recommended to execute this manifest in
# dry-run test mode. This validates there are no syntax errors and prints a
# report of what actions would have been performed. To execute in dry-run mode:
#  sudo puppet apply --noop nm-cfg.pp
#
# To apply the manifest (see paragraph above):
#  sudo puppet apply nm-cfg.pp
#
# For more info, see:
#   https://fuchsia.dev/internal/intree/development/networkmanager-cfg
#
# Contact: hansens@google.com


# To be fully standalone, we'll inline the configuration file contents rather
# than rely on a local puppet module-base. The file contents are given as a
# string value. In the future, it's possible these files could be read off
# fuchsia.googlesource.com rather than inlined here.
$nm_connection = @(EOF/L)
  # A connection profile for Fuchsia CDC devices.

  # The UUID needs to be unique among all connection profiles.
  #   $ python3 -c \'import uuid; print(uuid.uuid4())\'
  #
  # The multi-connect parameter may need tuning around your
  # usecase. This configures for 3 concurrent connections.
  #
  # See man 5 nm-settings for more info on what each individual
  # parameter does.

  [connection]
  id=Fuchsia CDC
  uuid=25b512f5-70e0-4fd3-9bc2-34432a01bc80
  permissions=
  type=ethernet
  autoconnect=true
  autoconnect-priority=-10
  multi-connect=3

  [match]
  interface-name=&zx-*

  [ipv4]
  method=link-local

  [ipv6]
  addr-gen-mode=eui64
  method=link-local
  | EOF


$nm_device = @(EOF/L)
  [device]
  match-device=interface-name:zx-*
  managed=1
  allowed-connections=uuid:25b512f5-70e0-4fd3-9bc2-34432a01bc80
  | EOF


$udev_rules = @(EOF/L)
  # Check for Google network devices being added
  ACTION=="add", SUBSYSTEM=="net", ATTRS{idVendor}=="18d1", GOTO="zircon_add_net_google"

  # Any additional adapter(s) whose interface should be configured. Uncomment
  # the following rule, and update ATTR{address} to the adapter's MAC. Reload
  # udev's configuration using:
  #   sudo udevadm control --reload && sudo udevadm trigger
  #ACTION=="add", SUBSYSTEM=="net", ATTR{address}=="XX:XX:XX:XX:XX:XX", GOTO="zircon_net_name"

  GOTO="zircon_end"

  # Check for Zircon CDC Ethernet devices being added
  LABEL="zircon_add_net_google"
  # CDC Ethernet-only configuration
  ATTRS{idProduct}=="a020", GOTO="zircon_net_name"
  # CDC Ethernet & USB Test Function composite configuration
  ATTRS{idProduct}=="a023", GOTO="zircon_net_name"
  # CDC Ethernet & ADB
  ATTRS{idProduct}=="a026", GOTO="zircon_net_name"
  # CDC Ethernet & USB VSOCK bridge
  ATTRS{idProduct}=="a029", GOTO="zircon_net_name"
  # CDC Ethernet & ADB & USB VSOCK bridge
  ATTRS{idProduct}=="a02b", GOTO="zircon_net_name"
  GOTO="zircon_end"

  # Set the interface name based on the MAC
  LABEL="zircon_net_name"
  IMPORT{builtin}="net_id"
  PROGRAM="/bin/sh -c 'echo $${ID_NET_NAME_MAC#enx}'", NAME="zx-%c"

  LABEL="zircon_end"
  | EOF


$flag_adapter_as_fuchsia_script = @(EOF/L)
  #!/bin/bash

  set -e

  function usage() {
    cat << EOM
  This script will list the curently attached network devices (which are not
  loopback) which are attached via a usb parent bus, prompt the user to choose one
  and when chosen, will generate a udev rule to rename the interface so that
  it can be used with the Fuchsia CDC Ethernet connection.
  EOM
    exit ${1-0}
  }

  function main() {
    while getopts 'h' opt; do
      case ${opt} in
        h) usage ;;
        *) usage 1 ;;
      esac
    done
    shift $((OPTIND - 1))

    local -r interfaces=$(ip -details -json link show | jq -r '
    .[] |
      if .linkinfo.info_kind // .link_type == "loopback" then
        empty
      elif .parentbus != "usb" then
        empty
      else
        "\(.ifname) \(.address)"
      end
      ')

    if [ -z "${interfaces}" ]; then
      echo "No valid interfaces found"
      exit 0
    fi


    nl -w2 <<< ${interfaces}
    read -p "Enter the number of the device you want to flag as a Fuchsia device: " choice

    mac_addr=$(sed -n "${choice}p" <<< ${interfaces} | awk '{print $2}')

    echo ""

    if [ -z "${mac_addr}" ]; then
      echo "Invalid interface choice"
      exit 0
    fi

    local -r mac_addr_clean=$(sed s/://g <<< ${mac_addr})

    local inet_rename=""
    inet_rename+='id_net_name=\$\${ID_NET_NAME_MAC#enx}; echo \$\${id_net_name:='
    inet_rename+="${mac_addr_clean}}"

    local -r file_contents=$(cat << EOM
  ACTION==\"add\", SUBSYSTEM==\"net\", ATTR{address}==\"${mac_addr}\", GOTO=\"custom_zircon_net_name\"

  # skip following block unless/until action above is triggered later
  GOTO=\"custom_zircon_end\"

  # Set the interface name based on the MAC
  LABEL=\"custom_zircon_net_name\"
  IMPORT{builtin}=\"net_id\"
  PROGRAM=\"/bin/sh -c '${inet_rename}'\", NAME=\"zx-%c\"
  GOTO=\"custom_zircon_end\"

  LABEL=\"custom_zircon_end\"
  EOM
    )

    local -r file_name="/etc/udev/rules.d/70-custom-adapter-${mac_addr_clean}.rules"

    cat << EOM

  About to write file ${file_name} with content:


  ${file_contents}

  EOM

    sudo touch ${file_name}
    sudo bash -c "echo \"${file_contents}\" > \"${file_name}\""
    sudo chmod 0755 ${file_name}

    echo "Reloading udev rules..."

    sudo udevadm control --reload
    sudo udevadm trigger

    cat << EOM


  Success! You may need to detach and re-attach your device for this to take effect

  EOM
  }

  main "$@"
  | EOF


# Manifest entry.
node default {
  if $facts[kernel] != 'Linux' {
    err('Can only be applied on a Linux system')
    break()
  }

  # Default file attributes (unless overridden).
  File {
    owner => root,
    group => root,
    mode  => '0644',
  }

  # Deploy the required udev rules.
  file { 'udev-rules':
    path    => '/etc/udev/rules.d/70-zircon-ethernet.rules',
    content => $udev_rules,
    notify  => [
      Exec['udev-reload'],
      Exec['udev-trigger'],
    ],
  }

  # Reload udev's configuration.
  exec { 'udev-reload':
    command     => '/usr/bin/env udevadm control --reload',
    refreshonly => true,
  }

  # Trigger a udev change action.
  exec { 'udev-trigger':
    require     => Exec['udev-reload'],
    command     => '/usr/bin/env udevadm trigger --action=change',
    refreshonly => true,
  }

  # Ensure NetworkManager is running. If NetworkManager is not installed, this
  # can be expected to fail.
  service { 'NetworkManager':
    ensure => running,
  }


  # Deploy script to flag a network interface as a Fuchsia one.
  file { 'flag-interface-as-fuchsia':
    path    => '/usr/local/bin/flag-adapter-as-fuchsia',
    content => $flag_adapter_as_fuchsia_script,
    mode    => '0755',
  }

  # Deploy the required NetworkManager connection profile.
  file { 'fuchsia_cdc.nmconnection':
    path    => '/etc/NetworkManager/system-connections/fuchsia_cdc.nmconnection',
    content => $nm_connection,
    mode    => '0600',
    notify  => Exec['nmcli-reload'],
  }

  # Deploy the required NetworkManager device configuration.
  file { 'fuchsia.conf':
    require => File['fuchsia_cdc.nmconnection'],
    path    => '/etc/NetworkManager/conf.d/fuchsia.conf',
    content => $nm_device,
    notify  => Exec['service-restart'],
  }

  # Reload NetworkManager's connection profiles.
  exec { 'nmcli-reload':
    require     => Service['NetworkManager'],
    command     => '/usr/bin/env nmcli connection reload',
    refreshonly => true,
  }

  # Restart the NetworkManager service.
  exec { 'service-restart':
    require     => Service['NetworkManager'],
    command     => '/usr/bin/env service NetworkManager restart',
    refreshonly => true,
  }
}
