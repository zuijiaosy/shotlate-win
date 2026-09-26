#!/bin/bash
# Cross-builds, copies Shotlate.exe into the VM, runs --check all and the end-to-end test on its desktop, and
# pulls the log and screenshots into target/vm-e2e/.
set -euo pipefail
cd "$(dirname "$0")/../.."
scripts/build-windows.sh >/dev/null
vm=scripts/vm
$vm/gx 'taskkill /IM Shotlate.exe /F >nul 2>&1 & mkdir C:\shotlate 2>nul' >/dev/null || true
utmctl file push "Shotlate Win11" 'C:\shotlate\Shotlate.exe' < dist/Shotlate.exe 2>/dev/null
$vm/guirun 'C:\shotlate\Shotlate.exe --check all C:\shotlate\check' 300
$vm/guirun 'C:\shotlate\Shotlate.exe --e2e C:\shotlate\e2e' 900
out=target/vm-e2e; mkdir -p "$out"
names="$($vm/gx 'dir /b C:\shotlate\e2e' | tr -d '\r' | grep -E '\.(png|log)$')"
for f in $names; do utmctl file pull "Shotlate Win11" "C:\\shotlate\\e2e\\$f" > "$out/$f" 2>/dev/null; done
cat "$out/e2e.log"
echo "screenshots: $out"
