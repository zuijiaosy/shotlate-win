#!/bin/bash
# Builds the real installer inside the VM (Inno Setup there) from a fresh cross-build, and installs it for
# "tester" like a user would (desktop shortcut, app started). Settings and downloaded models are kept.
#   scripts/vm/install.sh [--fresh]   --fresh also removes settings, API key and models first
set -euo pipefail
cd "$(dirname "$0")/../.."
vm=scripts/vm
version="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
scripts/build-windows.sh >/dev/null
ws="${TMPDIR:-/tmp}/winsparkle"
if [ ! -f "$ws/x64.dll" ]; then
  mkdir -p "$ws" && curl -fsSL -o "$ws/ws.zip" https://github.com/vslavik/winsparkle/releases/download/v0.9.4/WinSparkle-0.9.4.zip
  unzip -qo "$ws/ws.zip" -d "$ws" && cp "$ws"/WinSparkle-*/x64/Release/WinSparkle.dll "$ws/x64.dll"
fi
$vm/gx 'schtasks /end /tn ShotlateRun >nul 2>&1 & taskkill /IM Shotlate.exe /F >nul 2>&1 & rmdir /s /q C:\build\dist 2>nul & mkdir C:\build\installer C:\build\res C:\build\dist\x64 2>nul' >/dev/null || true
push() { utmctl file push "Shotlate Win11" "$2" < "$1" 2>/dev/null; }
push dist/Shotlate.exe 'C:\build\dist\x64\Shotlate.exe'
push "$ws/x64.dll" 'C:\build\dist\x64\WinSparkle.dll'
push installer/shotlate.iss 'C:\build\installer\shotlate.iss'
push res/shotlate.ico 'C:\build\res\shotlate.ico'
printf '@echo off\r\ncd /d C:\\build\\installer\r\n"C:\\Program Files (x86)\\Inno Setup 6\\ISCC.exe" /Q /DAppVersion=%s /DArch=x64 "/DSourceDir=..\\dist\\x64" shotlate.iss\r\n' "$version" > "${TMPDIR:-/tmp}/build.cmd"
push "${TMPDIR:-/tmp}/build.cmd" 'C:\build\build.cmd'
$vm/gx 'C:\build\build.cmd' >/dev/null
if [ "${1:-}" = "--fresh" ]; then
  $vm/gx 'rmdir /s /q C:\Users\tester\AppData\Roaming\Shotlate 2>nul & rmdir /s /q C:\Users\tester\AppData\Local\Shotlate 2>nul' >/dev/null
fi
$vm/guirun "C:\\build\\dist\\Shotlate-$version-x64.exe /SILENT /SUPPRESSMSGBOXES" 180 >/dev/null
$vm/gx 'schtasks /end /tn ShotlateRun >nul 2>&1 & dir /b C:\build\dist\*.exe & dir /b C:\Users\tester\Desktop & tasklist | findstr /i shotlate'
