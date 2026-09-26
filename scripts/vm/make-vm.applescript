-- Creates the "Shotlate Win11" UTM VM: Windows 11 ARM64, 6 cores, 8 GB, 64 GB NVMe, the Windows ISO and the
-- setup ISO (Autounattend.xml + UTM guest tools) attached. See scripts/vm/setup.sh.
on run argv
	set iso to POSIX file (item 1 of argv)
	set setup to POSIX file (item 2 of argv)
	with timeout of 900 seconds
		tell application "UTM"
			set vm to make new virtual machine with properties {backend:qemu, configuration:{name:"Shotlate Win11", notes:"Windows 11 ARM64 for testing Shotlate (user tester / shotlate)", architecture:"aarch64", memory:8192, cpu cores:6, hypervisor:true, uefi:true, drives:{{interface:NVMe, guest size:65536}, {removable:true, interface:USB, source:iso}, {removable:true, interface:USB, source:setup}}, network interfaces:{{hardware:"virtio-net-pci", mode:shared}}, displays:{{hardware:"virtio-ramfb", dynamic resolution:true}}}}
			start vm
			-- "Press any key to boot from CD or DVD"
			repeat 25 times
				input keystroke vm text " "
				delay 0.5
			end repeat
			return id of vm
		end tell
	end timeout
end run
