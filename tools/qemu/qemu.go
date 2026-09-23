// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package qemu

import (
	"errors"
	"fmt"
	"strings"
)

const (
	DefaultNetwork   = "10.0.2.0/24"
	DefaultDHCPStart = "10.0.2.15"
	DefaultGateway   = "10.0.2.2"
	DefaultDNS       = "10.0.2.3"
)

// Config holds emulator configuration values which can be passed to
// `ffx emu start` as a file through the --config flag.
// TODO(ihuh): Move to a less-QEMU specific package since it can be used
// for any type of emulation engine with `ffx emu start`.
type Config struct {
	Args       []string          `json:"args"`
	Envs       map[string]string `json:"envs"`
	Features   []string          `json:"features"`
	KernelArgs []string          `json:"kernel_args"`
	Options    []string          `json:"options"`
}

type Drive struct {
	// TODO(kjharland): Embed `Device` here.

	// ID is the block device identifier.
	ID string

	// File is the disk image file.
	File string

	// Addr is the PCI address of the block device.
	Addr string
}

type Chardev struct {
	// ID is the character device identifier.
	ID string

	// Logfile is a path to write the output to.
	Logfile string

	// Signal controls whether signals are enabled on the terminal.
	Signal bool
}

type Forward struct {
	// HostPort is the port on the host.
	HostPort int

	// GuestPort is the port on the guest.
	GuestPort int
}

type Netdev struct {
	Device

	// User is a netdev user backend.
	User *NetdevUser

	// Tap is a netdev tap backend.
	Tap *NetdevTap

	// ID is the network device identifier.
	ID string
}

type RNGdev struct {
	Device

	// ID is the RNG device identifier.
	ID string

	// Filename is the file name of the RNG.
	Filename string
}

type Device struct {
	Model   string
	options []string
}

type DeviceModel string

// DeviceModel constants.
const (
	DeviceModelVirtioBlkPCI = "virtio-blk-pci"
	DeviceModelVirtioNetPCI = "virtio-net-pci"
	DeviceModelRNG          = "virtio-rng-device"
)

func (d *Device) AddOption(key, val string) {
	d.options = append(d.options, fmt.Sprintf("%s=%s", key, val))
}

func (d *Device) String() string {
	return fmt.Sprintf("%s,%s", d.Model, strings.Join(d.options, ","))
}

// NetdevUser defines a netdev backend giving user networking.
type NetdevUser struct {
	// Network is the network block.
	Network string

	// DHCPStart is the address at which the DHCP allocation starts.
	DHCPStart string

	// DNS is the address of the builtin DNS server.
	DNS string

	// Host is the host IP address.
	Host string

	// Forwards are the host forwardings.
	Forwards []Forward
}

// HCI identifies a Host Controller Interface.
type HCI string

// Host Controller Interface constants.
const (
	// XHCI is the Extensible Host Controller Interface.
	// https://en.wikipedia.org/wiki/Extensible_Host_Controller_Interface
	XHCI = "xhci"
)

// NetdevTap defines a netdev backend giving a tap interface.
type NetdevTap struct {
	// Name is the name of the interface.
	Name string
}

// Target is a QEMU architecture target.
type Target string

type targetList struct {
	AArch64 Target
	X86_64  Target
	RiscV64 Target
}

// TargetEnum provides accessors to valid QEMU target strings.
var TargetEnum = &targetList{
	AArch64: "aarch64",
	X86_64:  "x86_64",
	RiscV64: "riscv64",
}

type uefiVolumes struct {
	rom        string
	nvram      string
	filesystem string
}

// QEMUCommandBuilder provides methods to construct an arbitrary
// QEMU invocation, it does not validate inputs only that the
// resulting invocation is structurely valid.
type QEMUCommandBuilder struct {
	args       []string
	qemuPath   string
	hasNetwork bool
	initrd     string
	kernel     string
	uefi       *uefiVolumes
	kernelArgs []string
	hasDisk    bool
	forAEMU    bool // For AEMU-specific quirks

	// Any errors encountered while building the command.
	errs []string
}

func (q *QEMUCommandBuilder) SetFlag(args ...string) {
	q.args = append(q.args, args...)
}

func (q *QEMUCommandBuilder) SetBinary(qemuPath string) {
	q.qemuPath = qemuPath
}

func (q *QEMUCommandBuilder) SetKernel(kernel string) {
	q.kernel = kernel
}

func (q *QEMUCommandBuilder) SetInitrd(initrd string) {
	q.initrd = initrd
}

// SetUEFIVolumes specifies the two storage volumes necessary for booting
// through UEFI - the ROM image implementing UEFI itself and a NVRAM image
// containing the EFI global variable store - as well as an optional,
// bootable (FAT) filesystem image containing the desired application.
func (q *QEMUCommandBuilder) SetUEFIVolumes(rom, nvram, filesystem string) {
	q.uefi = &uefiVolumes{rom, nvram, filesystem}
}

func (q *QEMUCommandBuilder) SetTarget(target Target, kvm bool) {
	switch target {
	case TargetEnum.AArch64:
		if kvm {
			q.SetFlag("-machine", "virt-9.2,gic-version=host,highmem-ecam=off")
			q.SetFlag("-cpu", "host")
			q.SetFlag("-enable-kvm")
		} else {
			q.SetFlag("-machine", "virt-9.2,gic-version=3,virtualization=true,highmem-ecam=off")
			q.SetFlag("-accel", "tcg,thread=multi")
			q.SetFlag("-cpu", "max")
		}
	case TargetEnum.X86_64:
		q.AddKernelArg("kernel.serial=legacy")
		// AEMU's fork of QEMU does not support the smbios-entry-point-type
		// machine property, but supports the type the kernel does without any
		// added qualification.
		if q.forAEMU {
			q.SetFlag("-machine", "q35")
		} else {
			q.SetFlag("-machine", "q35,smbios-entry-point-type=32")
		}

		// Override the SeaBIOS serial port to keep it from outputting
		// a terminal reset on start.
		q.SetFlag("-fw_cfg", "name=etc/sercon-port,string=0")

		if kvm {
			q.SetFlag("-cpu", "host,migratable=no,+invtsc")
			q.SetFlag("-enable-kvm")
		} else {
			q.SetFlag("-cpu", "Skylake-Client,-check")
			q.SetFlag("-accel", "tcg,thread=multi")
		}
	case TargetEnum.RiscV64:
		if kvm {
			q.recordError(fmt.Errorf("KVM is not supported for RISC-V 64 bit yet"))
		} else {
			q.SetFlag("-cpu", "rv64,svpbmt=true,v=true,vext_spec=v1.0")
			q.SetFlag("-machine", "virt")
			q.SetFlag("-accel", "tcg,thread=multi")
		}
	default:
		q.recordError(fmt.Errorf("invalid target: %q", target))
	}
}

func (q *QEMUCommandBuilder) SetMemory(memoryBytes int) {
	q.SetFlag("-m", fmt.Sprintf("%d", memoryBytes))
}

func (q *QEMUCommandBuilder) SetCPUCount(cpuCount int) {
	q.SetFlag("-smp", fmt.Sprintf("%d", cpuCount))
}

func (q *QEMUCommandBuilder) AddVirtioBlkPciDrive(d Drive) {
	q.hasDisk = true
	iothread := fmt.Sprintf("iothread-%s", d.ID)
	q.SetFlag("-object", fmt.Sprintf("iothread,id=%s", iothread))
	q.SetFlag("-drive", fmt.Sprintf("id=%s,file=%s,format=raw,if=none,cache=unsafe,aio=threads", d.ID, d.File))
	device := fmt.Sprintf("virtio-blk-pci,drive=%s,iothread=%s", d.ID, iothread)
	if d.Addr != "" {
		device += fmt.Sprintf(",addr=%s", d.Addr)
	}
	q.SetFlag("-device", device)
}

func (q *QEMUCommandBuilder) AddUSBDrive(d Drive) {
	q.SetFlag("-drive", fmt.Sprintf("if=none,id=%s,file=%s,format=raw", d.ID, d.File))
	q.SetFlag("-device", fmt.Sprintf("usb-storage,drive=%s,removable=on", d.ID))
}

func (q *QEMUCommandBuilder) AddUFSDrive(d Drive) {
	q.SetFlag("-device", "ufs")
	q.SetFlag("-drive", fmt.Sprintf("if=none,id=%s,file=%s,format=raw,discard=unmap", d.ID, d.File))
	q.SetFlag("-device", fmt.Sprintf("ufs-lu,drive=%s,lun=0", d.ID))
}

// AddHCI adds an host-controller-interface.
func (q *QEMUCommandBuilder) AddHCI(hci HCI) {
	if hci != XHCI {
		q.recordError(fmt.Errorf("unimplemented host controller interface: %q", hci))
		return
	}
	q.SetFlag("-device", "qemu-xhci,id=xhci")
}

func (q *QEMUCommandBuilder) AddSerial(c Chardev) {
	var builder strings.Builder
	builder.WriteString(fmt.Sprintf("stdio,id=%s", c.ID))
	if c.Logfile != "" {
		builder.WriteString(fmt.Sprintf(",logfile=%s", c.Logfile))
	}
	if !c.Signal {
		builder.WriteString(",signal=off")
	}
	q.SetFlag("-chardev", builder.String())
	device := fmt.Sprintf("chardev:%s", c.ID)
	q.SetFlag("-serial", device)
}

func (q *QEMUCommandBuilder) AddNetwork(n Netdev) {
	var network strings.Builder
	if n.Tap != nil {
		// Overwrite any default configuration scripts with none; there is not currently a
		// good use case for these parameters.
		network.WriteString(fmt.Sprintf("tap,id=%s,ifname=%s,script=no,downscript=no", n.ID, n.Tap.Name))
	} else if n.User != nil {
		network.WriteString(fmt.Sprintf("user,id=%s", n.ID))
		if n.User.Network != "" {
			network.WriteString(fmt.Sprintf(",net=%s", n.User.Network))
		}
		if n.User.DHCPStart != "" {
			network.WriteString(fmt.Sprintf(",dhcpstart=%s", n.User.DHCPStart))
		}
		if n.User.DNS != "" {
			network.WriteString(fmt.Sprintf(",dns=%s", n.User.DNS))
		}
		if n.User.Host != "" {
			network.WriteString(fmt.Sprintf(",host=%s", n.User.Host))
		}
		for _, f := range n.User.Forwards {
			network.WriteString(fmt.Sprintf(",hostfwd=tcp::%d-:%d", f.HostPort, f.GuestPort))
		}
	}
	q.SetFlag("-netdev", network.String())
	n.Device.AddOption("netdev", n.ID)
	q.SetFlag("-device", n.Device.String())

	q.hasNetwork = true
}

func (q *QEMUCommandBuilder) AddRNG(r RNGdev) {
	q.SetFlag("-object", fmt.Sprintf("rng-random,filename=%s,id=%s", r.Filename, r.ID))
	r.AddOption("rng", r.ID)
	q.SetFlag("-device", r.String())
}

func (q *QEMUCommandBuilder) AddKernelArg(kernelArg string) {
	q.kernelArgs = append(q.kernelArgs, kernelArg)
}

func (q *QEMUCommandBuilder) validate() error {
	if len(q.errs) == 1 {
		return errors.New(q.errs[0])
	}
	if len(q.errs) > 0 {
		return fmt.Errorf("multiple errors: [\n%s\n]", strings.Join(q.errs, ",\n"))
	}
	if q.kernel == "" && q.uefi == nil {
		return fmt.Errorf("one of QEMU kernel path or a UEFI disk image must be set.")
	}
	uefiDiskBoot := q.uefi != nil && q.uefi.filesystem != ""
	if uefiDiskBoot && (q.kernel != "" || q.initrd != "") {
		return fmt.Errorf("specifying a UEFI filesystem/disk image is mutually exclusive with QEMU kernel and initrd.")
	}
	return nil
}

// Build creates a QEMU invocation given a particular configuration, a list of
// images, and any specified command-line arguments.
func (q *QEMUCommandBuilder) Build() ([]string, error) {
	if q.qemuPath == "" {
		return []string{}, fmt.Errorf("QEMU binary path must be set.")
	}
	config, err := q.BuildConfig()
	if err != nil {
		return []string{}, err
	}

	cmd := []string{q.qemuPath}
	cmd = append(cmd, config.Args...)

	if len(config.KernelArgs) > 0 {
		cmd = append(cmd, "-append", strings.Join(config.KernelArgs, " "))
	}

	return cmd, nil
}

// BuildConfig returns a Config storing the emulator configuration values.
// It can be written to a file and passed through the --config flag to `ffx emu start`.
func (q *QEMUCommandBuilder) BuildConfig() (Config, error) {
	var config Config
	if err := q.validate(); err != nil {
		return config, err
	}
	config.Envs = make(map[string]string)
	config.Features = []string{}
	if q.kernel != "" {
		config.Args = append(config.Args, "-kernel", q.kernel)
	}
	if q.initrd != "" {
		config.Args = append(config.Args, "-initrd", q.initrd)
	}
	config.KernelArgs = []string{}
	if q.uefi != nil {
		config.Args = append(
			config.Args,
			"-drive",
			fmt.Sprintf("if=pflash,format=raw,readonly=on,file=%s", q.uefi.rom),
			// `snapshot=true`` ensures that this image is copy-on-write and that
			// state does not persist across invocations.
			"-drive",
			fmt.Sprintf("if=pflash,format=raw,snapshot=on,file=%s", q.uefi.nvram),
		)

		if q.uefi.filesystem != "" {
			config.Args = append(
				config.Args, "-drive",
				fmt.Sprintf("if=none,format=raw,file=%s,id=uefi", q.uefi.filesystem),
			)
			// `bootindex=0` ensures that the provided storage device is
			// regarded as the highest priority boot option.
			bootindex := ""
			if q.kernel == "" {
				bootindex = ",bootindex=0"
			}
			// If no disk is present, put the filesystem image on a USB drive.
			if q.hasDisk {
				config.Args = append(
					config.Args,
					"-device",
					fmt.Sprintf("virtio-blk-pci,drive=uefi%s", bootindex),
				)
			} else {
				config.Args = append(
					config.Args,
					"-device",
					"nec-usb-xhci,id=xhci0",
					"-device",
					fmt.Sprintf("usb-storage,bus=xhci0.0,drive=uefi,removable=on%s", bootindex),
				)
			}
		}
	}
	config.Args = append(config.Args, q.args...)
	config.Options = []string{}

	// Treat the absence of specified networks as a directive to disable networking entirely.
	if !q.hasNetwork {
		config.Args = append(config.Args, "-net", "none")
	}

	// `-append` requires `-kernel`.
	if q.kernel != "" {
		config.KernelArgs = q.kernelArgs
	}

	return config, nil
}

func (q *QEMUCommandBuilder) recordError(err error) {
	q.errs = append(q.errs, err.Error())
}
