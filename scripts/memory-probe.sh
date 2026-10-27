#!/usr/bin/env bash
# Measure steady-state memory of the riced frontend and bisect what costs it.
#
# The matrix isolates one variable per run so a regression can be attributed:
#
#   native   no Lua app at all: the renderer plus the native shell (control)
#   empty    one Lua layer, a single text child
#   slot     the same layer embedding riced.plugin("desktop-widgets"), no
#            placements (measures the desktop-widget host, not the widgets)
#   widgets  slot plus one `rectangle` placement (measures widget rendering)
#   churn    widgets, then N source reloads, measured again (leak detection)
#
# Nothing is written into the repository: every project is generated under a
# temp directory. Compare runs with the same number of connected outputs, and
# against the pre-migration binary if you still have it.
#
# Usage:
#   scripts/memory-probe.sh [--binary PATH] [--seconds N] [--reloads N] [--keep]
#
# Run inside the development shell so the runtime libraries resolve:
#   nix develop --command scripts/memory-probe.sh
set -euo pipefail

binary="./target/debug/riced"
seconds=8
reloads=20
keep=0

while [ $# -gt 0 ]; do
    case "$1" in
        --binary) binary="$2"; shift 2 ;;
        --seconds) seconds="$2"; shift 2 ;;
        --reloads) reloads="$2"; shift 2 ;;
        --keep) keep=1; shift ;;
        -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [ ! -x "$binary" ]; then
    echo "riced binary not found at $binary (build it, or pass --binary)" >&2
    exit 1
fi
if [ -z "${WAYLAND_DISPLAY:-}" ]; then
    echo "WAYLAND_DISPLAY is unset; run this inside a Wayland session" >&2
    exit 1
fi
name="$(basename "$binary")"
if pgrep -x "$name" >/dev/null 2>&1 || pgrep -x riced >/dev/null 2>&1; then
    echo "a riced daemon is already running; stop it first (one per session)" >&2
    exit 1
fi

root="$(mktemp -d "${TMPDIR:-/tmp}/riced-memory-XXXXXX")"
cleanup() {
    if [ "$keep" = 1 ]; then
        echo "projects kept in $root"
    else
        rm -rf "$root"
    fi
}
trap cleanup EXIT

rss_kb() { awk '/^VmRSS:/{print $2}' "/proc/$1/status" 2>/dev/null || echo 0; }
threads() { awk '/^Threads:/{print $2}' "/proc/$1/status" 2>/dev/null || echo 0; }

# One generated project: config.lua plus an app whose view is $1.
make_project() {
    local dir="$1" view="$2" desktop="${3:-}"
    mkdir -p "$dir/apps/probe"
    cat > "$dir/config.lua" <<LUA
return {
    apps = { { name = "probe" } },
    background = { disabled = true },
    theme = { name = "gruvbox", templates_dir = "off" },
    desktop = { $desktop },
    bar = {},
}
LUA
    cat > "$dir/apps/probe/app.lua" <<LUA
return {
    view = function(self, props)
        $view
    end,
}
LUA
}

# The same project without the Lua app: the native background shell. This is the
# control for "did the Lua frontend add cost, or was it always there?".
make_native_project() {
    local dir="$1"
    mkdir -p "$dir"
    cat > "$dir/config.lua" <<'LUA'
return {
    apps = {},
    theme = { name = "gruvbox", templates_dir = "off" },
    bar = {},
}
LUA
}

layer_with() {
    cat <<LUA
return iced.layer({
            id = "probe",
            layer = "background",
            anchors = { top = true, right = true, bottom = true, left = true },
            exclusive_zone = -1,
            keyboard = "none",
            input = "all",
            pointer_events = true,
            content = $1,
        })
LUA
}

# Launch one project, sample until steady, print a line, stop it.
measure() {
    local label="$1" dir="$2" reload_after="${3:-0}"
    local log="$root/$label.log"
    "$binary" -p "$dir" >"$log" 2>&1 &
    local pid=$!
    local peak=0 samples=0 last=0
    for _ in $(seq 1 $((seconds * 2))); do
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "$label: exited early; see $log" >&2
            sed -n '1,5p' "$log" >&2
            return 1
        fi
        local now
        now="$(rss_kb "$pid")"
        [ "$now" -gt "$peak" ] && peak="$now"
        last="$now"
        samples=$((samples + 1))
        sleep 0.5
    done
    if [ "$reload_after" -gt 0 ]; then
        for _ in $(seq 1 "$reload_after"); do
            touch "$dir/apps/probe/app.lua"
            sleep 0.2
        done
        sleep 2
        last="$(rss_kb "$pid")"
        [ "$last" -gt "$peak" ] && peak="$last"
    fi
    printf '%-9s steady %6s MB   peak %6s MB   threads %3s   (%s samples)\n' \
        "$label" "$((last / 1024))" "$((peak / 1024))" "$(threads "$pid")" "$samples"
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
}

text_layer="$(layer_with 'iced.text("probe " .. props.output.name)')"
slot_layer="$(layer_with 'riced.plugin("desktop-widgets")')"
widgets_layer="$(layer_with 'riced.plugin("desktop-widgets")')"

echo "binary: $binary"
echo "projects: $root"
echo
outputs="?"
if command -v wayland-info >/dev/null 2>&1; then
    outputs="$(wayland-info 2>/dev/null | grep -c 'interface: .wl_output' || true)"
fi
echo "outputs: $outputs"
echo
echo "note: build profile dominates RSS. A debug binary maps ~96 MB of segments"
echo "      against ~40 MB for release, and ~68 MB of the debug image is resident."
echo "      Always compare like for like (--binary)."
echo

make_native_project "$root/native"
measure native "$root/native"

make_project "$root/empty" "$text_layer"
measure empty "$root/empty"

make_project "$root/slot" "$slot_layer"
measure slot "$root/slot"

make_project "$root/widgets" "$widgets_layer" \
    '{ output = "", widget = "rectangle", x = 40.0, y = 40.0, width = 240.0, height = 160.0 }'
measure widgets "$root/widgets"

make_project "$root/churn" "$widgets_layer" \
    '{ output = "", widget = "rectangle", x = 40.0, y = 40.0, width = 240.0, height = 160.0 }'
measure churn "$root/churn" "$reloads"

cat <<'NOTE'

Reading the numbers:
  native -> empty    what the Lua frontend adds over the native shell, same
                     binary and same renderer
  empty  -> slot     the desktop-widget host itself (retain + views per render)
  slot   -> widgets  widget rendering and its per-instance Lua state
  widgets -> churn   retention across reloads; a large gap is a leak

Two costs have nothing to do with Lua and usually dominate:
  - the build profile: compare --binary target/debug/riced against
    --binary ~/.local/bin/riced (release)
  - the renderer: the Vulkan ICDs pull in libLLVM plus a driver per vendor.
    Force the CPU fallback with VK_ICD_FILENAMES=/dev/null.

Measured here (2 outputs, same projects, so the deltas are comparable):
  debug    native 111 MB   empty 117 MB
  release  native  77 MB   empty  80 MB   <- profile costs ~34 MB
  release, VK_ICD_FILENAMES=/dev/null:
           native  61 MB   empty  64 MB   <- renderer costs ~16 MB
  The Lua frontend is ~3 MB (release) / ~6 MB (debug). Slot, widgets and 20
  reloads all land inside the noise floor.

Deltas under ~5 MB are noise: raise --seconds or repeat runs before reading
anything into them. Keep the output count identical across runs.
NOTE
