#!/bin/sh
# Bootstrap only: native Zork owns installation, services and mesh enrollment.
set -eu

fail() { printf '%s\n' "Zork: $*" >&2; exit 1; }
fetch() {
    curl --fail --silent --show-error --location --retry 2 --connect-timeout 15 \
        --max-time 600 --proto "$fetch_protocols" --proto-redir "$redirect_protocols" "$1" -o "$2"
}
version_at_least() {
    awk -v actual="$1" -v minimum="$2" 'BEGIN {
        split(actual,a,"."); split(minimum,b,".");
        for(i=1;i<=3;i++) {if(a[i]+0>b[i]+0) exit 0; if(a[i]+0<b[i]+0) exit 1} exit 0
    }'
}

main() {
    release_version=latest
    download_only=
    allow_http_test=
    fetch_protocols='=https,file'
    redirect_protocols='=https'
    base_url=${ZORK_RELEASE_BASE_URL:-https://github.com/HOOLC/zork/releases}
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --download-only) [ "$#" -ge 2 ] || fail '--download-only requires a directory'; download_only=$2; shift 2 ;;
            --version) [ "$#" -ge 2 ] || fail '--version requires a value'; release_version=$2; shift 2 ;;
            --base-url) [ "$#" -ge 2 ] || fail '--base-url requires a value'; base_url=$2; shift 2 ;;
            --allow-http-test) allow_http_test=1; shift ;;
            --help|-h) printf '%s\n' 'Usage: install.sh [--version VERSION] [--base-url RELEASES_URL] [-- NATIVE_ARGS...]' 'Default: install and start a persistent Station. Example: -- mesh join INVITATION --data DIR'; return ;;
            --) shift; break ;;
            *) fail "Unknown installer option: $1 (put native arguments after --)" ;;
        esac
    done
    [ "$#" -gt 0 ] || set -- install
    if [ -n "$allow_http_test" ]; then
        test_channel=
        previous=
        for argument in "$@"; do
            if [ "$previous" = --channel ]; then test_channel=$argument; fi
            previous=$argument
        done
        [ "$test_channel" = test ] || fail '--allow-http-test requires --channel test'
        fetch_protocols='=https,http,file'
        redirect_protocols='=https,http'
    fi
    # Reuse a compatible installation without requiring a download or restarting tasks.
    if [ "$1" = mesh ] && [ "${2:-}" = join ] && command -v zork >/dev/null 2>&1; then
        if zork capabilities 2>/dev/null | grep -Eq '"mesh_join"[[:space:]]*:[[:space:]]*1'; then
            zork "$@"
            return
        fi
    fi
    for tool in curl tar awk mktemp; do command -v "$tool" >/dev/null 2>&1 || fail "Missing prerequisite: $tool"; done
    case "$base_url" in https://*|file:///*) ;; http://*) [ -n "$allow_http_test" ] || fail 'HTTP requires --allow-http-test and --channel test' ;; *) fail 'Release source must use HTTPS (or file:/// for offline installation)' ;; esac
    base_url=${base_url%/}
    case "$(uname -s)" in Darwin) os=darwin ;; Linux) os=linux ;; *) fail 'Supported systems: macOS and Linux' ;; esac
    case "$(uname -m)" in arm64|aarch64) arch=arm64 ;; x86_64|amd64) arch=x64 ;; *) fail 'Supported CPUs: ARM64 and x64' ;; esac
    if command -v sha256sum >/dev/null 2>&1; then hash_tool=sha256sum
    elif command -v shasum >/dev/null 2>&1; then hash_tool=shasum
    else fail 'Missing SHA-256 tool (sha256sum or shasum)'; fi
    scratch=$(mktemp -d "${TMPDIR:-/tmp}/zork-install.XXXXXXXX")
    trap 'rm -rf "$scratch"' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM HUP
    if [ "$release_version" = latest ]; then
        fetch "$base_url/latest/download/VERSION" "$scratch/VERSION"
        release_version=$(cat "$scratch/VERSION")
    fi
    release_version=${release_version#v}
    printf '%s\n' "$release_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.-]+)?$' || fail 'Invalid release version'
    release_url="$base_url/download/v$release_version"
    fetch "$release_url/manifest.tsv" "$scratch/manifest.tsv"
    row=$(awk -F '\t' -v key="$os-$arch" '$1==key {print; count++} END {if(count!=1) exit 1}' "$scratch/manifest.tsv") || fail "Release does not contain exactly one package for $os-$arch"
    archive=$(printf '%s\n' "$row" | awk -F '\t' '{print $2}')
    digest=$(printf '%s\n' "$row" | awk -F '\t' '{print $3}')
    minimum=$(printf '%s\n' "$row" | awk -F '\t' '{print $4}')
    [ "$archive" = "zork-$release_version-$os-$arch.tar.gz" ] || fail 'Invalid package filename'
    [ "${#digest}" -eq 64 ] || fail 'Invalid package checksum'
    case "$digest" in *[!0-9a-f]*) fail 'Invalid package checksum' ;; esac
    printf '%s\n' "$minimum" | grep -Eq '^[0-9]+\.[0-9]+(\.[0-9]+)?$' || fail 'Invalid minimum system version'
    if [ "$os" = darwin ]; then
        actual=$(sw_vers -productVersion)
        version_at_least "$actual" "$minimum" || fail "macOS $minimum or later is required (found $actual)"
    else
        actual=$(getconf GNU_LIBC_VERSION 2>/dev/null) || fail 'This release requires glibc Linux; musl/Alpine is not supported'
        actual=${actual##* }
        version_at_least "$actual" "$minimum" || fail "glibc $minimum or later is required (found $actual)"
    fi
    printf 'Downloading Zork %s (%s-%s)…\n' "$release_version" "$os" "$arch" >&2
    fetch "$release_url/$archive" "$scratch/package.tar.gz"
    if [ "$hash_tool" = shasum ]; then actual_digest=$(shasum -a 256 "$scratch/package.tar.gz" | awk '{print $1}')
    else actual_digest=$(sha256sum "$scratch/package.tar.gz" | awk '{print $1}'); fi
    [ "$actual_digest" = "$digest" ] || fail 'Package SHA-256 mismatch; nothing installed'
    # Exact flat member list; reject links and special files before extraction.
    tar -tzf "$scratch/package.tar.gz" > "$scratch/members"
    awk 'BEGIN {split("zork zork-station zork-agent zork-gh VERSION LICENSE Synchronicity.txt",names," "); for(i in names) expected[names[i]]=1}
         !($0 in expected) || seen[$0]++ {exit 1} END {if(NR!=7) exit 1}' "$scratch/members" || fail 'Unexpected package contents'
    tar -tvzf "$scratch/package.tar.gz" > "$scratch/types"
    awk 'substr($0,1,1)!="-" {exit 1}' "$scratch/types" || fail 'Package contains links or special files'
    mkdir "$scratch/bin"
    tar -xzf "$scratch/package.tar.gz" -C "$scratch/bin"
    [ "$(cat "$scratch/bin/VERSION")" = "$release_version" ] || fail 'Package version mismatch'
    for component in zork zork-station zork-agent zork-gh; do
        [ -x "$scratch/bin/$component" ] || fail "Missing executable: $component"
    done
    if [ -n "$download_only" ]; then
        [ ! -e "$download_only" ] || fail 'Download destination already exists'
        mkdir "$download_only"
        cp -p "$scratch/bin/"* "$download_only/"
        return
    fi
    "$scratch/bin/zork" "$@"
}

# Keep execution after the full function definition for piped installation.
main "$@"
