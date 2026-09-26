#!/bin/bash
# Records the README / website media in the VM with the installed Shotlate:
#   docs/images/capture-flow.gif, docs/images/settings.gif, and target/demo/ (mp4, webp poster, frames).
# The capture shortcut is set to the default (Alt+Shift+A) for the recording and the user's settings are restored after.
set -euo pipefail
cd "$(dirname "$0")/../.."
V=scripts/vm
VM="Shotlate Win11"
APPDIR='C:\Users\tester\AppData\Roaming\Shotlate'
EXE='C:\Users\tester\AppData\Local\Programs\Shotlate\Shotlate.exe'
OUT=target/demo
mkdir -p "$OUT" docs/images

scripts/build-windows.sh >/dev/null
utmctl file push "$VM" 'C:\shotlate\Shotlate.exe' < dist/Shotlate.exe

# The app is launched through explorer: started from run.cmd it would inherit (and lock) guirun's output file.
utmctl file pull "$VM" "$APPDIR\\settings.json" > "$OUT/settings.user.json"
python3 - "$OUT/settings.user.json" "$OUT/settings.demo.json" <<'EOF'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
d["captureShortcut"] = {"vk": 65, "modifiers": 5}
open(sys.argv[2], "w", encoding="utf-8").write(json.dumps(d, ensure_ascii=False, indent=2))
EOF
$V/gx "taskkill /IM Shotlate.exe /F >nul 2>&1 & copy /y $APPDIR\\settings.json $APPDIR\\settings.json.bak" >/dev/null
# Put the user's settings back however this script ends (including Ctrl+C or a timeout).
restore() {
  $V/gx "taskkill /IM Shotlate.exe /F >nul 2>&1 & copy /y $APPDIR\\settings.json.bak $APPDIR\\settings.json & del $APPDIR\\settings.json.bak" >/dev/null || true
  $V/guirun "explorer.exe \"$EXE\"" 30 >/dev/null || true
}
trap restore EXIT
utmctl file push "$VM" "$APPDIR\\settings.json" < "$OUT/settings.demo.json"
$V/guirun "explorer.exe \"$EXE\"" 30 >/dev/null
sleep 5
$V/gx 'rmdir /s /q C:\shotlate\demo 2>nul & echo ok' >/dev/null
$V/guirun 'C:\shotlate\Shotlate.exe --e2e C:\shotlate\demo demo' 300 >/dev/null || true
utmctl file pull "$VM" 'C:\shotlate\demo\e2e.log' | tee "$OUT/e2e.log"

restore
trap - EXIT
grep -q "DONE 0 failed" "$OUT/e2e.log" || { echo "demo failed, media not updated" >&2; exit 1; }

utmctl file pull "$VM" 'C:\shotlate\demo\frames.ffconcat' > "$OUT/frames.ffconcat"
for f in $(awk '/^file/ {print $2}' "$OUT/frames.ffconcat" | sort -u) demo-ocr.png settings-0.png settings-1.png settings-2.png settings-3.png; do
  utmctl file pull "$VM" "C:\\shotlate\\demo\\$f" > "$OUT/$f"
done

cd "$OUT"
# The action happens around Notepad at (150, 90); crop to it.
CROP="crop=880:480:140:84"
ffmpeg -v error -y -f concat -safe 0 -i frames.ffconcat \
  -vf "$CROP,fps=15,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle" \
  -loop 0 capture-flow.gif
ffmpeg -v error -y -f concat -safe 0 -i frames.ffconcat -vf "$CROP,fps=30,format=yuv420p" -c:v libx264 -crf 22 -preset slow -movflags +faststart -an capture-flow.mp4
ffmpeg -v error -y -i demo-ocr.png -vf "$CROP" poster.png
cwebp -quiet -q 85 poster.png -o capture-flow-poster.webp
{
  echo "ffconcat version 1.0"
  for i in 0 1 2 3; do printf 'file settings-%s.png\nduration 1.8\n' "$i"; done
  echo "file settings-3.png"
} > settings.ffconcat
ffmpeg -v error -y -f concat -safe 0 -i settings.ffconcat -vf "fps=10,split[a][b];[a]palettegen=max_colors=96[p];[b][p]paletteuse=dither=none" -loop 0 settings.gif
cwebp -quiet -q 88 settings-2.png -o settings-translate.webp
cp capture-flow.gif settings.gif ../../docs/images/
ls -la capture-flow.gif settings.gif capture-flow.mp4 capture-flow-poster.webp settings-translate.webp
