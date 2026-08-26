#!/usr/bin/env bash
set -euo pipefail

assets_dir="${1:-release-assets}"
dmg=$(find "$assets_dir" -name '*.dmg' -print -quit)
test -n "$dmg"

require_universal() {
  local binary=$1
  local label=$2
  local architectures
  architectures=$(lipo -archs "$binary")
  case " $architectures " in
    *" arm64 "*) ;;
    *) echo "$label is missing its Apple Silicon slice: $architectures" >&2; exit 1 ;;
  esac
  case " $architectures " in
    *" x86_64 "*) ;;
    *) echo "$label is missing its Intel slice: $architectures" >&2; exit 1 ;;
  esac
  printf 'Verified universal %s (%s)\n' "$label" "$architectures"
}

mount_point=$(mktemp -d)
app_pid=''
recorder_pid=''
cleanup() {
  if [ -n "$recorder_pid" ]; then
    kill "$recorder_pid" 2>/dev/null || true
    wait "$recorder_pid" 2>/dev/null || true
  fi
  if [ -n "$app_pid" ]; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  hdiutil detach "$mount_point" -quiet || true
  rmdir "$mount_point" || true
}
trap cleanup EXIT

hdiutil attach "$dmg" -mountpoint "$mount_point" -nobrowse -readonly -quiet
app="$mount_point/Polar Stream.app"
app_binary="$app/Contents/MacOS/polar-stream"
info_plist="$app/Contents/Info.plist"
liblsl="$app/Contents/Resources/liblsl.dylib"
recorder_app=$(find "$app/Contents/Resources" -type d -path '*/lab-recorder/LabRecorder.app' -print -quit)
test -x "$app_binary"
test -f "$info_plist"
test -f "$liblsl"
test -n "$recorder_app"

require_universal "$app_binary" 'Polar Stream executable'
require_universal "$liblsl" 'bundled liblsl'
require_universal "$recorder_app/Contents/MacOS/LabRecorder" 'bundled LabRecorder executable'
require_universal "$recorder_app/Contents/Frameworks/lsl.framework/lsl" 'LabRecorder liblsl framework'
codesign --verify --deep --strict "$app"
plutil -extract NSBluetoothAlwaysUsageDescription raw "$info_plist" | grep -q 'Bluetooth'

"$app_binary" > smoke.log 2>&1 &
app_pid=$!
sleep 8
kill -0 "$app_pid" || { cat smoke.log; exit 1; }

recorder_root=$(dirname "$recorder_app")
"$recorder_app/Contents/MacOS/LabRecorder" -c "$recorder_root/PolarStream-LabRecorder.cfg" > recorder-smoke.log 2>&1 &
recorder_pid=$!
sleep 6
kill -0 "$recorder_pid" || { cat recorder-smoke.log; exit 1; }

printf 'Verified mounted DMG launch on %s\n' "$(uname -m)"
