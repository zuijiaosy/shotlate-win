#!/bin/bash
# One-time: a Windows 11 ARM64 test VM in UTM, installed unattended (user tester / password shotlate,
# Simplified Chinese with Microsoft Pinyin, UTM guest tools). Needs `brew install --cask utm`, ~30 GB and ~30 min.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
vms="$HOME/VMs"; mkdir -p "$vms"
iso="$vms/Win11_zh-cn_Arm64.iso"
if [ ! -f "$iso" ]; then
  url="$(python3 "$here/windows-iso-url.py" 3324 "Chinese (Simplified)" | awk '{print $2}' | head -1)"
  curl -fL --retry 5 -C - -o "$iso" "$url"
fi
tools="$vms/utm-guest-tools.iso"
[ -f "$tools" ] || curl -fsSL -o "$tools" https://getutm.app/downloads/utm-guest-tools-latest.iso
cd_dir="$vms/setup-cd"; rm -rf "$cd_dir"; mkdir -p "$cd_dir"
hdiutil attach -nobrowse -readonly -mountpoint /tmp/utm-gt "$tools" >/dev/null
cp -R /tmp/utm-gt/Drivers /tmp/utm-gt/utm-guest-tools-*.exe "$cd_dir/"
exe="$(basename /tmp/utm-gt/utm-guest-tools-*.exe)"
hdiutil detach /tmp/utm-gt >/dev/null
sed "s/utm-guest-tools-0.1.271.exe/$exe/g" "$here/Autounattend.xml" > "$cd_dir/Autounattend.xml"
rm -f "$vms/shotlate-setup.iso"
hdiutil makehybrid -quiet -iso -joliet -default-volume-name SHOTLATE_SETUP -o "$vms/shotlate-setup.iso" "$cd_dir"
osascript "$here/make-vm.applescript" "$iso" "$vms/shotlate-setup.iso"
echo "Installing; when 'scripts/vm/gx \"type C:\\shotlate\\setup-done.txt\"' prints done, the VM is ready."
