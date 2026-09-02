#!/usr/bin/env bash
# The release battery, as one command: three cargo lanes in parallel, the two
# script checks, and the by-name test-set difference against a saved baseline.
#
# It exists because the battery was until now a recipe in prose that each
# release re-typed by hand, and a lane typed by hand is a lane that can be
# forgotten. The calibration lane is the one that was forgotten most often:
# the corpus-gated tests print a skip line and PASS when
# AUTOSHADE_FIT_CALIBRATION_DIR is unset, so a battery that never sets it is
# green for the wrong reason. This script refuses to pretend: no corpus, no
# calibration lane, exit 1 with the reason.
#
# The transcript is written in `=== name ===` blocks so
# `scripts/check_docs.py --gates <transcript>` can read the test counts and
# the calibration lane straight out of it.
#
#   scripts/release_battery.sh                       # all three lanes
#   scripts/release_battery.sh --out /tmp/b.txt      # transcript elsewhere
#   scripts/release_battery.sh --save-baseline       # re-pin the name list
#   scripts/release_battery.sh --baseline names.txt  # diff against that list
#
# Environment (all optional except the first for the calibration lane):
#   AUTOSHADE_FIT_CALIBRATION_DIR  the p36-p39 corpus; required, checked
#   AUTOSHADE_CENSUS_ROOT          passed through for the doc gate's census
#   AUTOSHADE_SEGMENT_SCRIPT       defaulted to <repo>/python/segment.py
#   AUTOSHADE_CORRESPOND_SCRIPT    defaulted to <repo>/python/correspond.py
#   BATTERY_TARGET_ROOT            parent of the three CARGO_TARGET_DIRs
#   BATTERY_DATA_ROOT              parent of the three AUTOSHADE_DATA_DIRs
#
# Windows note: Git Bash paths are converted with `cygpath -w` before they are
# handed to cargo, because cargo.exe reads CARGO_TARGET_DIR as a native path
# and would take `/d/t/x` for a directory called `d` on the current drive.
#
# GPU note: the sidecar script paths reach ALL THREE lanes, so a machine with
# weights configured can have the default lane and the calibration lane load a
# model at the same time. Measured on an 8 GB card with the v1.2.4 suite: both
# lanes completed, twice, the calibration lane taking 822 s against the default
# lane's 446 s because its corpus-gated tests actually run. On a smaller card,
# run the lanes one at a time by passing a different --out per lane rather than
# by editing this script.

set -u
set -o pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)
OUT="$REPO/target/battery/transcript.txt"
BASELINE="$REPO/target/battery/base-tests.txt"
SAVE_BASELINE=0

while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT=$2; shift 2 ;;
        --baseline) BASELINE=$2; shift 2 ;;
        --save-baseline) SAVE_BASELINE=1; shift ;;
        -h|--help) sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "release_battery: unknown argument $1" >&2; exit 2 ;;
    esac
done

TARGET_ROOT=${BATTERY_TARGET_ROOT:-$REPO/target/battery/t}
DATA_ROOT=${BATTERY_DATA_ROOT:-$REPO/target/battery/data}

# ---------------------------------------------------------------- the corpus
CORPUS=${AUTOSHADE_FIT_CALIBRATION_DIR:-}
if [ -z "$CORPUS" ]; then
    echo "release_battery: AUTOSHADE_FIT_CALIBRATION_DIR is unset, so the" >&2
    echo "  calibration lane cannot run. Every corpus-gated test would print" >&2
    echo "  a skip line and pass, and the battery would be green without" >&2
    echo "  having measured anything. Point it at the p36-p39 corpus." >&2
    exit 1
fi
if [ ! -d "$CORPUS" ]; then
    echo "release_battery: AUTOSHADE_FIT_CALIBRATION_DIR=$CORPUS is not a" >&2
    echo "  directory. The calibration lane needs the p36-p39 pairs; without" >&2
    echo "  them its tests skip and pass, which is not a lane." >&2
    exit 1
fi

# ------------------------------------------------------------------ helpers
native() {
    # Cargo and the sidecars are native Windows binaries under Git Bash.
    if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf '%s' "$1"; fi
}

mkdir -p "$(dirname "$OUT")" "$TARGET_ROOT" "$DATA_ROOT"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/autoshade-battery.XXXXXX") || exit 1
trap 'rm -rf "$WORK"' EXIT

export AUTOSHADE_SEGMENT_SCRIPT=${AUTOSHADE_SEGMENT_SCRIPT:-$(native "$REPO/python/segment.py")}
export AUTOSHADE_CORRESPOND_SCRIPT=${AUTOSHADE_CORRESPOND_SCRIPT:-$(native "$REPO/python/correspond.py")}
[ -n "${AUTOSHADE_CENSUS_ROOT:-}" ] && export AUTOSHADE_CENSUS_ROOT

lane() {
    # lane <name> <extra cargo args...> — one cargo test in its own target and
    # data dir, output to $WORK/<name>.txt, exit status to $WORK/<name>.rc.
    name=$1
    shift
    mkdir -p "$TARGET_ROOT/$name" "$DATA_ROOT/$name"
    (
        export CARGO_TARGET_DIR=$(native "$TARGET_ROOT/$name")
        export AUTOSHADE_DATA_DIR=$(native "$DATA_ROOT/$name")
        if [ "$name" = calib ]; then
            export AUTOSHADE_FIT_CALIBRATION_DIR=$(native "$CORPUS")
        else
            unset AUTOSHADE_FIT_CALIBRATION_DIR
        fi
        cd "$REPO" || exit 1
        # `${1+"$@"}` and not `"$@"`: macOS still ships bash 3.2, where an
        # empty `"$@"` under `set -u` is an unbound-variable error.
        cargo test --offline --release ${1+"$@"} 2>&1
        echo $? > "$WORK/$name.rc"
    ) > "$WORK/$name.txt" 2>&1
}

# --------------------------------------------------------------- three lanes
# The gui feature only adds dependencies, so the gui lane runs the gui bin
# alone; the calibration lane runs the library a second time with the corpus
# in reach and --nocapture, which is the only way a skip line reaches the
# transcript at all (libtest swallows a passing test's stderr).
lane default &
PID_DEFAULT=$!
lane gui --features gui --bin autoshade-gui &
PID_GUI=$!
lane calib --lib -- --nocapture &
PID_CALIB=$!
wait $PID_DEFAULT $PID_GUI $PID_CALIB

rc_of() { cat "$WORK/$1.rc" 2>/dev/null || echo 1; }
RC_DEFAULT=$(rc_of default)
RC_GUI=$(rc_of gui)
RC_CALIB=$(rc_of calib)

# ------------------------------------------------------- the two script gates
CHECKS="$WORK/checks.txt"
{
    echo "\$ python scripts/audit_i18n.py"
    (cd "$REPO" && python scripts/audit_i18n.py 2>&1)
    echo "exit $?"
    echo
    echo "\$ python scripts/subset_gui_fonts.py --check"
    (cd "$REPO" && python scripts/subset_gui_fonts.py --check 2>&1)
    echo "exit $?"
} > "$CHECKS" 2>&1

# ------------------------------------------------------------- the name diff
NAMES="$WORK/names.txt"
(
    cd "$REPO" || exit 1
    export CARGO_TARGET_DIR=$(native "$TARGET_ROOT/default")
    export AUTOSHADE_DATA_DIR=$(native "$DATA_ROOT/default")
    cargo test --offline --release --lib -- --list 2>/dev/null
) | grep ': test$' | sort > "$NAMES"
NAME_COUNT=$(wc -l < "$NAMES" | tr -d ' ')

DIFF="$WORK/diff.txt"
ADDED=0
REMOVED=0
if [ "$SAVE_BASELINE" = 1 ]; then
    mkdir -p "$(dirname "$BASELINE")"
    cp "$NAMES" "$BASELINE"
    echo "baseline saved: $BASELINE ($NAME_COUNT names)" > "$DIFF"
elif [ -f "$BASELINE" ]; then
    ADDED=$(comm -13 "$BASELINE" "$NAMES" | wc -l | tr -d ' ')
    REMOVED=$(comm -23 "$BASELINE" "$NAMES" | wc -l | tr -d ' ')
    {
        echo "baseline: $BASELINE"
        comm -13 "$BASELINE" "$NAMES" | sed 's/^/+ /'
        comm -23 "$BASELINE" "$NAMES" | sed 's/^/- /'
    } > "$DIFF"
else
    echo "no baseline at $BASELINE — run with --save-baseline on the base commit" > "$DIFF"
fi

# ------------------------------------------------------------- the transcript
SKIPS=$(grep -c '^SKIPPED ' "$WORK/calib.txt" 2>/dev/null || true)
[ -z "$SKIPS" ] && SKIPS=0
FAILED=0
for rc in "$RC_DEFAULT" "$RC_GUI" "$RC_CALIB"; do
    [ "$rc" = 0 ] || FAILED=$((FAILED + 1))
done

{
    echo "=== battery ==="
    echo "repo:    $REPO"
    echo "commit:  $(cd "$REPO" && git rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo "date:    $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    echo "host:    $(uname -s) $(uname -m)"
    echo "corpus:  $CORPUS"
    echo "census:  ${AUTOSHADE_CENSUS_ROOT:-(unset)}"
    echo
    echo "=== test default ==="
    cat "$WORK/default.txt"
    echo
    echo "=== test gui ==="
    cat "$WORK/gui.txt"
    echo
    echo "=== test calib ==="
    cat "$WORK/calib.txt"
    echo
    echo "=== checks ==="
    cat "$CHECKS"
    echo
    echo "=== test names ==="
    cat "$DIFF"
    echo
    echo "=== summary ==="
    echo "lane default: exit $RC_DEFAULT"
    echo "lane gui: exit $RC_GUI"
    echo "lane calib: exit $RC_CALIB"
    echo "calib skipped: $SKIPS"
    echo "test names: $NAME_COUNT (+$ADDED -$REMOVED)"
    echo "lanes failed: $FAILED"
} > "$OUT"

sed -n '/^=== summary ===$/,$p' "$OUT"
echo
echo "transcript: $OUT"
[ "$FAILED" = 0 ] || exit 1
exit 0
