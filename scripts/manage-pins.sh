#!/usr/bin/env bash
#
# manage-pins.sh — inspect and update the toolchain versions this repo pins.
#
# For every pinned tool this prints the version pinned in the repo (read from
# the repo files themselves, so it can't drift from what CI/Docker actually
# use), the latest version available upstream, whether the pin is current,
# and whether all files that reference the pin agree with each other. It also
# scans the repo for pin definitions that live outside the managed files, so
# a stray pin can't go unnoticed.
#
# `update` rewrites every occurrence of a pin in one go, so bumping a tool
# (e.g. Trunk) updates the workflow AND the Dockerfile together.
#
# Commands:
#   list                Print the pins, latest upstream versions, and
#                       cross-file consistency (default).
#   check               Same as list, but exits 1 if any pin disagrees across
#                       files or lives in a file this script doesn't manage.
#                       With --strict also exits 1 when an update is
#                       available (handy for CI).
#   update <tool> [ver] Bump <tool>'s pin in every file that pins it. With no
#                       version it uses the latest available upstream (needs
#                       network). Pass a version explicitly to pick one.
#   update --all [-y]   Bump every tool to its latest upstream version.
#                       Prompts for confirmation unless -y/--yes is given.
#   help                Show this help text (same as -h/--help).
#
# Options:
#   --offline           Don't query upstream; print "—" for latest.
#   --dry-run           Show the exact diffs without writing anything.
#   -y, --yes           Skip confirmation prompts.
#   --strict            (check) treat "update available" as a failure.
#   -h, --help          Show usage.
#
# Tools tracked: rust trunk sccache node wix dotnet playwright
#
# Pin locations:
#   rust       rust-toolchain.toml (channel), Dockerfile (ARG RUST_VERSION)
#   trunk      .github/workflows/rust.yml (TRUNK_VERSION), Dockerfile (ARG TRUNK_VERSION)
#   sccache    .github/actions/install-system-deps/action.yml (SCCACHE_VERSION)
#   node       .github/workflows/rust.yml (node-version)
#   wix        .github/workflows/rust.yml (wix --version)
#   dotnet     .github/workflows/rust.yml (-Channel)
#   playwright tests/e2e + scripts/ package.json + package-lock.json (npm-managed)
#
# Override JUMBIE_ROOT to point at a different checkout (useful for testing).
# Exit codes: 0 ok · 1 check found problems · 2 usage/argument error.

set -uo pipefail

ROOT="${JUMBIE_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
TOOLS="rust trunk sccache node wix dotnet playwright"

OFFLINE=0
DRY_RUN=0
YES=0
STRICT=0
ALL=0

usage() {
    local end
    end=$(grep -n '^set -uo pipefail' "$0" | head -1 | cut -d: -f1)
    [ -n "$end" ] || end=45
    sed -n "2,$((end - 1))p" "$0" | sed 's/^# \{0,1\}//'
}

# ── version helpers ──────────────────────────────────────────────────────

ver_num() { # strip "v" prefix and any non-numeric suffix (empty if not numeric)
    printf '%s' "${1#v}" | sed -nE 's/^([0-9]+(\.[0-9]+)*).*/\1/p'
}

ver_gt() { # 0 if $1 > $2 (numeric dotted comparison; false if either side isn't numeric)
    local a b ai bi i n
    a=$(ver_num "$1")
    b=$(ver_num "$2")
    [ -n "$a" ] && [ -n "$b" ] || return 1
    IFS='.' read -ra ai <<<"$a"
    IFS='.' read -ra bi <<<"$b"
    n=$(( ${#ai[@]} > ${#bi[@]} ? ${#ai[@]} : ${#bi[@]} ))
    for (( i = 0; i < n; i++ )); do
        if (( ${ai[$i]:-0} > ${bi[$i]:-0} )); then return 0; fi
        if (( ${ai[$i]:-0} < ${bi[$i]:-0} )); then return 1; fi
    done
    return 1
}

valid_version() { # rough sanity check for version strings we write into files
    [[ "$1" =~ ^[0-9][A-Za-z0-9._+-]*$ ]]
}

# ── pin locations ────────────────────────────────────────────────────────
# Each entry is: FILE|REGEX|DESCRIPTION
# REGEX has exactly three groups: (prefix)(optional "v")(version).
#   reading: group 3 is the version   writing: groups 1+2 are kept as-is.

locations_of() {
    case "$1" in
    rust)
        printf '%s\n' \
            "rust-toolchain.toml|^(channel[[:space:]]*=[[:space:]]*\")(v?)([^\"]*)|channel = \"…\"" \
            "Dockerfile|^(ARG RUST_VERSION=)(v?)([0-9][^[:space:]]*)|ARG RUST_VERSION=…"
        ;;
    trunk)
        printf '%s\n' \
            ".github/workflows/rust.yml|^([[:space:]]*TRUNK_VERSION: )(v?)([0-9][^[:space:]]*)|TRUNK_VERSION: …" \
            "Dockerfile|^(ARG TRUNK_VERSION=)(v?)([0-9][^[:space:]]*)|ARG TRUNK_VERSION=…"
        ;;
    sccache)
        printf '%s\n' \
            ".github/actions/install-system-deps/action.yml|^([[:space:]]*SCCACHE_VERSION=\")(v?)([0-9][^\"]*)|SCCACHE_VERSION=\"…\""
        ;;
    node)
        printf '%s\n' \
            ".github/workflows/rust.yml|^([[:space:]]*node-version: )(v?)([0-9][^[:space:]]*)|node-version: …"
        ;;
    wix)
        printf '%s\n' \
            ".github/workflows/rust.yml|(wix --version[[:space:]]*)(v?)([0-9][^[:space:]]*)|wix --version …"
        ;;
    dotnet)
        printf '%s\n' \
            ".github/workflows/rust.yml|(-Channel[[:space:]]*)(v?)([0-9][^[:space:]]*)|-Channel …"
        ;;
    playwright)
        printf '%s\n' \
            "tests/e2e/package-lock.json|jq|@playwright/test (lockfile)" \
            "tests/e2e/package.json|jq|@playwright/test (range)" \
            "scripts/package-lock.json|jq|playwright (lockfile)" \
            "scripts/package.json|jq|playwright (range)"
        ;;
    esac
}

# ── reading pins ─────────────────────────────────────────────────────────

read_location() { # "FILE|REGEX|DESC" → version (group 3 of the regex)
    local file regex
    file="${1%%|*}"
    regex=$(printf '%s' "$1" | cut -d'|' -f2)
    [ -f "$ROOT/$file" ] || return 1
    # wrap the regex so the whole line is replaced by group 3, not just the match
    sed -nE "s|.*$regex.*|\3|p" "$ROOT/$file" | head -1
}

pw_version() { # $1=dir → playwright version locked in that dir's package-lock.json
    local lock="$ROOT/$1/package-lock.json" v
    [ -f "$lock" ] || return 1
    if command -v jq >/dev/null 2>&1; then
        v=$(jq -r '.packages["node_modules/@playwright/test"].version // .packages["node_modules/playwright"].version // empty' "$lock" 2>/dev/null)
    elif command -v python3 >/dev/null 2>&1; then
        v=$(python3 -c 'import json,sys
d = json.load(open(sys.argv[1]))
p = d["packages"]
print(p.get("node_modules/@playwright/test", p.get("node_modules/playwright", {})).get("version", ""))' "$lock" 2>/dev/null)
    else
        v=$(sed -nE '/"node_modules\/@playwright\/test"|"node_modules\/playwright"/,/^[[:space:]]*}/ s/^[[:space:]]*"version": "([^"]*)"/\1/p' "$lock" | head -1)
    fi
    printf '%s' "$v"
}

pw_range() { # $1=dir → playwright range from that dir's package.json (^/ ~ stripped)
    local pj="$ROOT/$1/package.json" v
    [ -f "$pj" ] || return 1
    if command -v jq >/dev/null 2>&1; then
        v=$(jq -r '(.dependencies["@playwright/test"] // .devDependencies["@playwright/test"] // .dependencies["playwright"] // .devDependencies["playwright"] // empty)' "$pj" 2>/dev/null)
    else
        v=$(grep -oE '"@playwright/test"[[:space:]]*:[[:space:]]*"[^"]*"|"playwright"[[:space:]]*:[[:space:]]*"[^"]*"' "$pj" 2>/dev/null | head -1 | sed -E 's/.*"([^"]*)"$/\1/')
    fi
    printf '%s' "$v" | sed -E 's/^[\^~]//'
}

read_playwright() { # canonical playwright pin: version locked in tests/e2e
    pw_version tests/e2e
}

read_pin() { # $1=tool → canonical pinned version
    local loc v
    if [ "$1" = playwright ]; then
        read_playwright
        return $?
    fi
    while IFS= read -r loc; do
        [ -z "$loc" ] && continue
        v=$(read_location "$loc")
        if [ -n "$v" ]; then
            printf '%s' "$v"
            return 0
        fi
    done < <(locations_of "$1")
    return 1
}

# ── latest versions (upstream) ───────────────────────────────────────────

latest_rust() {
    [ "$OFFLINE" = 1 ] && return 1
    curl -fsSL --max-time 20 https://static.rust-lang.org/dist/channel-rust-stable.toml 2>/dev/null |
        awk '/^\[/ { if ($0=="[pkg.rust]") f=1; else if (f) exit }
             f && /^version[[:space:]]*=/ { gsub(/[^0-9.]/,"",$3); print $3; exit }'
}

latest_github() { # $1=owner/repo → newest release tag, "v" stripped
    [ "$OFFLINE" = 1 ] && return 1
    local j
    j=$(curl -fsSL --max-time 20 "https://api.github.com/repos/$1/releases/latest" 2>/dev/null) || return 1
    [ -n "$j" ] || return 1
    if command -v jq >/dev/null 2>&1; then
        printf '%s' "$j" | jq -r '.tag_name' 2>/dev/null | sed 's/^v//'
    else
        printf '%s' "$j" | grep -o '"tag_name": *"[^"]*"' | head -1 | sed -E 's/.*"([^"]*)"$/\1/' | sed 's/^v//'
    fi
}

latest_trunk()   { latest_github trunk-rs/trunk; }
latest_sccache() { latest_github mozilla/sccache; }

latest_node() { # $1=pinned major → line 1: newest in that major, line 2: newest overall
    [ "$OFFLINE" = 1 ] && return 1
    local major="$1" idx
    idx=$(curl -fsSL --max-time 20 https://nodejs.org/dist/index.json 2>/dev/null) || return 1
    [ -n "$idx" ] || return 1
    if command -v jq >/dev/null 2>&1; then
        printf '%s' "$idx" | jq -r --arg m "v${major}." \
            '[([.[] | select(.version | startswith($m)) | .version][0] // empty), .[0].version] | .[] | select(. != null) | .[1:]' 2>/dev/null
    elif command -v python3 >/dev/null 2>&1; then
        printf '%s' "$idx" | python3 -c 'import json,sys
d = json.load(sys.stdin)
m = "v" + sys.argv[1] + "."
in_major = [e["version"] for e in d if e["version"].startswith(m)]
print(in_major[0][1:] if in_major else "")
print(d[0]["version"][1:])' "$major" 2>/dev/null
    fi
}

latest_wix() {
    [ "$OFFLINE" = 1 ] && return 1
    local idx
    idx=$(curl -fsSL --max-time 20 https://api.nuget.org/v3-flatcontainer/wix/index.json 2>/dev/null) || return 1
    [ -n "$idx" ] || return 1
    if command -v jq >/dev/null 2>&1; then
        printf '%s' "$idx" | jq -r '[.versions[] | select(test("-") | not)] | sort_by(split(".") | map(tonumber)) | last' 2>/dev/null
    elif command -v python3 >/dev/null 2>&1; then
        printf '%s' "$idx" | python3 -c 'import json,sys
d = json.load(sys.stdin)
v = [x for x in d["versions"] if "-" not in x]
print(max(v, key=lambda s: [int(p) for p in s.split(".")]))' 2>/dev/null
    fi
}

latest_dotnet() { # $1=pinned channel → line 1: newest SDK in channel, line 2: newest stable major
    [ "$OFFLINE" = 1 ] && return 1
    local channel="$1" idx
    idx=$(curl -fsSL --max-time 20 https://dotnetcli.blob.core.windows.net/dotnet/release-metadata/releases-index.json 2>/dev/null) || return 1
    [ -n "$idx" ] || return 1
    if command -v jq >/dev/null 2>&1; then
        printf '%s' "$idx" | jq -r --arg c "$channel" \
            '([.["releases-index"][] | select(."channel-version" == $c) | ."latest-sdk"][0] // empty),
             ([.["releases-index"][] | select(."support-phase" != "preview") | ."channel-version" | split(".")[0] | tonumber] | max | tostring) | select(. != null)' 2>/dev/null
    elif command -v python3 >/dev/null 2>&1; then
        printf '%s' "$idx" | python3 -c 'import json,sys
d = json.load(sys.stdin)
idx = d["releases-index"]
sdk = next((e["latest-sdk"] for e in idx if e["channel-version"] == sys.argv[1]), "")
stable = [int(e["channel-version"].split(".")[0]) for e in idx if e.get("support-phase") != "preview"]
print(sdk)
print(max(stable) if stable else "")' "$channel" 2>/dev/null
    fi
}

latest_playwright() {
    [ "$OFFLINE" = 1 ] && return 1
    local j
    j=$(curl -fsSL --max-time 20 https://registry.npmjs.org/playwright/latest 2>/dev/null) || return 1
    [ -n "$j" ] || return 1
    if command -v jq >/dev/null 2>&1; then
        printf '%s' "$j" | jq -r '.version' 2>/dev/null
    else
        printf '%s' "$j" | grep -o '"version": *"[^"]*"' | head -1 | sed -E 's/.*"([^"]*)"$/\1/'
    fi
}

LATEST_VAL=""
OVERALL_VAL=""

latest_for() { # $1=tool, $2=pinned → sets LATEST_VAL / OVERALL_VAL
    LATEST_VAL=""
    OVERALL_VAL=""
    local line i=0
    case "$1" in
    node | dotnet)
        while IFS= read -r line; do
            [ -z "$line" ] && continue
            [ "$line" = null ] && continue
            if [ $i -eq 0 ]; then LATEST_VAL="$line"; else OVERALL_VAL="$line"; fi
            i=$((i + 1))
        done <<<"$(latest_$1 "$2")"
        ;;
    *) LATEST_VAL=$(latest_$1) ;;
    esac
}

# ── status / consistency ─────────────────────────────────────────────────

status_of() { # $1=tool $2=pinned $3=latest $4=latest-overall
    local tool="$1" pinned="$2" latest="$3" overall="$4"
    case "$tool" in
    node | dotnet)
        if [ -z "$overall" ]; then
            echo "? offline"
        elif ver_gt "${overall%%.*}" "${pinned%%.*}"; then
            echo "⚠ newer major: ${overall%%.*}"
        else
            echo "✓ up to date"
        fi
        ;;
    *)
        if [ -z "$latest" ]; then
            echo "? offline"
        elif ver_gt "$latest" "$pinned"; then
            echo "⚠ update available ($latest)"
        else
            echo "✓ up to date"
        fi
        ;;
    esac
}

consistency_for() { # $1=tool → prints a line, returns 0 ok / 1 mismatch
    local tool="$1"
    if [ "$tool" = playwright ]; then
        local canonical v dir loc bad=0
        canonical=$(pw_version tests/e2e)
        if [ -z "$canonical" ]; then
            echo "  ? playwright: could not read tests/e2e/package-lock.json"
            return 1
        fi
        while IFS= read -r loc; do
            [ -z "$loc" ] && continue
            dir="${loc%%|*}"
            case "$dir" in
            *package-lock.json) v=$(pw_version "${dir%/package-lock.json}") ;;
            *package.json)     v=$(pw_range "${dir%/package.json}") ;;
            *)                 continue ;;
            esac
            if [ -z "$v" ]; then
                echo "  ? playwright: no version found in $dir"
                bad=1
            elif [ "$v" != "$canonical" ]; then
                echo "  ✗ playwright: $dir has $v, expected $canonical"
                bad=1
            fi
        done < <(locations_of playwright)
        if [ $bad -eq 0 ]; then
            echo "  ✓ playwright: tests/e2e and scripts agree ($canonical)"
        fi
        return $bad
    fi
    local base="" v loc bad=0 f
    while IFS= read -r loc; do
        [ -z "$loc" ] && continue
        f="${loc%%|*}"
        v=$(read_location "$loc")
        if [ -z "$v" ]; then
            echo "  ? $tool: no version found in $f"
            bad=1
            continue
        fi
        if [ -z "$base" ]; then
            base="$v"
        elif [ "$v" != "$base" ]; then
            echo "  ✗ $tool: $f has $v, expected $base"
            bad=1
        fi
    done < <(locations_of "$tool")
    [ $bad -eq 0 ] && echo "  ✓ $tool: all locations agree ($base)"
    return $bad
}

print_locations() {
    echo
    echo "Where each pin lives:"
    local tool loc f d first
    for tool in $TOOLS; do
        first=1
        printf '  %-10s' "$tool:"
        while IFS= read -r loc; do
            [ -z "$loc" ] && continue
            f="${loc%%|*}"
            d=$(printf '%s' "$loc" | sed -E 's/^[^|]*\|[^|]*\|//')
            if [ $first -eq 1 ]; then
                printf ' %s (%s)' "$f" "$d"
                first=0
            else
                printf ', %s (%s)' "$f" "$d"
            fi
        done < <(locations_of "$tool")
        echo
    done
}

# ── stray-pin scan ────────────────────────────────────────────────────────
# Scans the repo for pin definitions that live outside the managed files
# (e.g. a new Dockerfile that hardcodes a version). Patterns match the pin
# *definition* forms only, so usages like ${{ env.TRUNK_VERSION }} don't count.

scan_patterns() {
    case "$1" in
    rust)       echo 'channel[[:space:]]*=[[:space:]]*"|ARG RUST_VERSION=|FROM rust:[0-9]' ;;
    trunk)      echo '^[[:space:]]*TRUNK_VERSION:|ARG TRUNK_VERSION=|trunk-rs/trunk/releases/download/v[0-9]' ;;
    sccache)    echo 'SCCACHE_VERSION=|sccache/releases/download/v[0-9]' ;;
    node)       echo 'node-version:' ;;
    wix)        echo 'wix --version[[:space:]]*[0-9]' ;;
    dotnet)     echo '-Channel[[:space:]]*[0-9]' ;;
    playwright) echo '"@playwright/test"[[:space:]]*:|"playwright"[[:space:]]*:' ;;
    esac
}

tracked_files() { # $1=tool → relative paths of the files that manage its pin
    local loc
    while IFS= read -r loc; do
        [ -z "$loc" ] && continue
        printf '%s\n' "${loc%%|*}"
    done < <(locations_of "$1")
}

is_tracked_file() { # $1=tool, $2=relative path → 0 if managed by that tool
    local f
    while IFS= read -r f; do
        [ "$f" = "$2" ] && return 0
    done < <(tracked_files "$1")
    return 1
}

stray_pins() { # prints warnings; returns 1 if any pin lives outside the managed files
    local tool pat f rel problems=0
    for tool in $TOOLS; do
        pat=$(scan_patterns "$tool")
        [ -z "$pat" ] && continue
        while IFS= read -r f; do
            [ -z "$f" ] && continue
            rel="${f#"$ROOT"/}"
            [ "$rel" = scripts/manage-pins.sh ] && continue
            if ! is_tracked_file "$tool" "$rel"; then
                problems=1
                echo "  ✗ $tool pin in unmanaged file: $rel"
                grep -nE "$pat" "$f" | head -3 | sed 's/^/      /'
            fi
        done < <(grep -rInE --exclude-dir=.git --exclude-dir=target --exclude-dir=node_modules \
            --exclude-dir=venv --exclude-dir=dist --exclude-dir=.tmp --exclude-dir=logs \
            --exclude-dir=data --exclude-dir=test-results --exclude-dir=playwright-report \
            "$pat" "$ROOT" 2>/dev/null | cut -d: -f1 | sort -u)
    done
    if [ $problems -eq 0 ]; then
        echo "  ✓ no pins outside the managed files"
    else
        echo "  (move them into the managed files, or add them to locations_of)"
    fi
    return $problems
}

# ── updating pins ────────────────────────────────────────────────────────

write_location() { # "FILE|REGEX|DESC" $2=new version (keeps prefix + optional "v")
    local file regex new="$2" tmp
    file="${1%%|*}"
    regex=$(printf '%s' "$1" | cut -d'|' -f2)
    [ -f "$ROOT/$file" ] || { echo "  ✗ $file not found"; return 1; }
    tmp=$(mktemp)
    sed -E "s|$regex|\1\2$new|" "$ROOT/$file" > "$tmp"
    if [ "$DRY_RUN" = 1 ]; then
        echo "  [dry-run] would update $file"
        diff -u --label "$file (current)" --label "$file (after)" "$ROOT/$file" "$tmp" || true
    else
        mv "$tmp" "$ROOT/$file"
        echo "  updated $file"
    fi
    rm -f "$tmp"
}

update_generic() { # $1=tool $2=new version
    local loc
    while IFS= read -r loc; do
        [ -z "$loc" ] && continue
        write_location "$loc" "$2"
    done < <(locations_of "$1")
}

update_playwright() { # $1=new version — npm-managed, bump via npm so the
    # lockfile and integrity hashes stay valid.
    local d pkg
    for d in tests/e2e scripts; do
        [ -f "$ROOT/$d/package.json" ] || continue
        pkg=""
        grep -q '"@playwright/test"' "$ROOT/$d/package.json" && pkg="@playwright/test@$1"
        grep -q '"playwright"' "$ROOT/$d/package.json" && pkg="${pkg:+$pkg }playwright@$1"
        [ -n "$pkg" ] || continue
        if [ "$DRY_RUN" = 1 ]; then
            echo "  [dry-run] would run: (cd $ROOT/$d && npm install --save-exact $pkg)"
        else
            echo "  running: (cd $ROOT/$d && npm install --save-exact $pkg)"
            ( cd "$ROOT/$d" && npm install --save-exact $pkg )
        fi
    done
}

resolve_new_version() { # $1=tool, $2=explicit version (may be empty)
    local tool="$1" explicit="$2"
    if [ -n "$explicit" ]; then
        explicit="${explicit#v}"
        valid_version "$explicit" || { echo "invalid version: $explicit" >&2; return 1; }
        printf '%s' "$explicit"
        return 0
    fi
    [ "$OFFLINE" = 1 ] && { echo "error: no version given (use --offline only with an explicit version)" >&2; return 1; }
    latest_for "$tool" "$(read_pin "$tool")"
    case "$tool" in
    node)   [ -n "$OVERALL_VAL" ] || { echo "error: could not fetch latest node version" >&2; return 1; }
            printf '%s' "${OVERALL_VAL%%.*}" ;;
    dotnet) [ -n "$OVERALL_VAL" ] || { echo "error: could not fetch latest .NET version" >&2; return 1; }
            printf '%s.0' "${OVERALL_VAL%%.*}" ;;
    *)      [ -n "$LATEST_VAL" ] || { echo "error: could not fetch latest $tool version (offline?)" >&2; return 1; }
            printf '%s' "$LATEST_VAL" ;;
    esac
}

all_at_version() { # $1=tool $2=version → 0 if every managed location already has it
    local loc v dir
    if [ "$1" = playwright ]; then
        while IFS= read -r loc; do
            [ -z "$loc" ] && continue
            dir="${loc%%|*}"
            case "$dir" in
            *package-lock.json) v=$(pw_version "${dir%/package-lock.json}") ;;
            *package.json)     v=$(pw_range "${dir%/package.json}") ;;
            *)                 continue ;;
            esac
            [ "$v" != "$2" ] && return 1
        done < <(locations_of playwright)
        return 0
    fi
    while IFS= read -r loc; do
        [ -z "$loc" ] && continue
        v=$(read_location "$loc")
        [ "$v" != "$2" ] && return 1
    done < <(locations_of "$1")
    return 0
}

cmd_update() { # $1=tool $2=explicit version (optional)
    local tool="$1" pinned newv
    case "$tool" in
    rust | trunk | sccache | node | wix | dotnet | playwright) ;;
    *) echo "unknown tool: $tool (valid: $TOOLS)"; return 2 ;;
    esac
    pinned=$(read_pin "$tool") || pinned="?"
    newv=$(resolve_new_version "$tool" "${2:-}") || return 1
    echo
    echo "== $tool: ${pinned} -> ${newv}"
    if all_at_version "$tool" "$newv"; then
        echo "  already pinned at $newv — nothing to do"
        return 0
    fi
    case "$tool" in
    playwright) update_playwright "$newv" ;;
    *)          update_generic "$tool" "$newv" ;;
    esac
}

cmd_update_all() {
    [ "$OFFLINE" = 1 ] && { echo "error: update --all needs network" >&2; return 1; }
    if [ "$YES" != 1 ] && [ "$DRY_RUN" != 1 ]; then
        echo "This will bump every pinned tool to its latest upstream version."
        printf 'Continue? [y/N] '
        read -r ans
        case "${ans:-}" in y | Y | yes | YES) ;; *) echo "aborted"; return 1 ;; esac
    fi
    local tool
    for tool in $TOOLS; do
        cmd_update "$tool"
    done
}

# ── reporting ────────────────────────────────────────────────────────────

cmd_list() {
    local tool pinned latest overall status problems=0
    echo
    echo "Pinned toolchain versions for Jumbie"
    if [ "$OFFLINE" = 1 ]; then
        echo "(offline — latest versions not queried)"
    else
        echo "(latest versions from upstream)"
    fi
    echo
    printf '%-10s %-11s %-16s %s\n' "Tool" "Pinned" "Latest" "Status"
    printf '%s\n' "---------- ----------- ---------------- ----------------------------------------"
    for tool in $TOOLS; do
        pinned=$(read_pin "$tool") || pinned="?"
        latest="—"
        overall=""
        if [ "$OFFLINE" != 1 ]; then
            latest_for "$tool" "$pinned"
            latest="$LATEST_VAL"
            overall="$OVERALL_VAL"
            [ -z "$latest" ] && latest="?"
        fi
        status=$(status_of "$tool" "$pinned" "$latest" "$overall")
        if [ "$OFFLINE" = 1 ]; then
            status="—"
        fi
        printf '%-10s %-11s %-16s %s\n' "$tool" "$pinned" "$latest" "$status"
        if [ "$STRICT" = 1 ] && [ "${status%% *}" = "⚠" ]; then
            problems=1
        fi
    done
    echo
    echo "Consistency:"
    for tool in $TOOLS; do
        consistency_for "$tool" || problems=1
    done
    echo
    echo "Stray pins:"
    stray_pins || problems=1
    print_locations
    echo
    echo "Legend: ✓ up to date · ⚠ newer version upstream · ? unknown (offline)."
    echo "For node/dotnet the pin is a major/channel, so ⚠ means a newer major is out."
    echo
    return $problems
}

# ── CLI ──────────────────────────────────────────────────────────────────

args=()
for a in "$@"; do
    case "$a" in
    --offline) OFFLINE=1 ;;
    --dry-run) DRY_RUN=1 ;;
    -y | --yes) YES=1 ;;
    --strict) STRICT=1 ;;
    --all) ALL=1 ;;
    -h | --help) usage; exit 0 ;;
    -*) echo "unknown option: $a" >&2; usage >&2; exit 2 ;;
    *) args+=("$a") ;;
    esac
done

cmd="${args[0]:-list}"

case "$cmd" in
list)
    cmd_list
    exit 0
    ;;
check)
    cmd_list
    exit $?
    ;;
help)
    usage
    exit 0
    ;;
update)
    if [ "$ALL" = 1 ]; then
        cmd_update_all
        exit $?
    fi
    if [ -z "${args[1]:-}" ]; then
        echo "error: update needs a tool (or --all)" >&2
        usage >&2
        exit 2
    fi
    cmd_update "${args[1]}" "${args[2]:-}"
    exit $?
    ;;
*)
    echo "unknown command: $cmd" >&2
    usage >&2
    exit 2
    ;;
esac
