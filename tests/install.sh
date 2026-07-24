#!/bin/sh

set -eu

repository=$(CDPATH= cd "$(dirname "$0")/.." && pwd -P)
test_directory=$(mktemp -d "${TMPDIR:-/tmp}/vrac-installer-test.XXXXXX")
trap 'rm -rf "$test_directory"' EXIT
trap 'exit 1' HUP INT TERM

case "$(uname -s):$(uname -m)" in
    Darwin:arm64) target="aarch64-apple-darwin" ;;
    Darwin:x86_64) target="x86_64-apple-darwin" ;;
    Linux:x86_64 | Linux:amd64) target="x86_64-unknown-linux-musl" ;;
    *)
        printf 'installer test does not support this platform\n' >&2
        exit 1
        ;;
esac

version="v0.1.0"
asset="vrac-$version-$target.tar.gz"
mkdir -p "$test_directory/assets" "$test_directory/package" \
    "$test_directory/fake-bin"
printf '#!/bin/sh\nprintf "vrac 0.1.0\\n"\n' \
    > "$test_directory/package/vrac"
chmod 755 "$test_directory/package/vrac"
cp "$repository/LICENSE" "$test_directory/package/LICENSE"
tar -C "$test_directory/package" -czf "$test_directory/assets/$asset" \
    vrac LICENSE

if command -v sha256sum >/dev/null 2>&1; then
    (cd "$test_directory/assets" && sha256sum "$asset" > "$asset.sha256")
else
    (cd "$test_directory/assets" && shasum -a 256 "$asset" > "$asset.sha256")
fi

printf '%s\n' \
    '#!/bin/sh' \
    'output=""' \
    'url=""' \
    'while [ "$#" -gt 0 ]; do' \
    '    case "$1" in' \
    '        --output) output=$2; shift 2 ;;' \
    '        https://*) url=$1; shift ;;' \
    '        *) shift ;;' \
    '    esac' \
    'done' \
    'printf "%s\\n" "$url" >> "$VRAC_INSTALLER_TEST_LOG"' \
    'case "$url" in' \
    '    */releases/latest)' \
    '        printf "%s" "https://github.com/yan-imensar/vrac/releases/tag/v0.1.0"' \
    '        ;;' \
    '    *)' \
    '        cp "$VRAC_INSTALLER_TEST_ASSETS/${url##*/}" "$output"' \
    '        ;;' \
    'esac' \
    > "$test_directory/fake-bin/curl"
chmod 755 "$test_directory/fake-bin/curl"

installer_log="$test_directory/curl.log"
install_directory="$test_directory/install/bin"
PATH="$test_directory/fake-bin:$PATH" \
VRAC_INSTALLER_TEST_ASSETS="$test_directory/assets" \
VRAC_INSTALLER_TEST_LOG="$installer_log" \
VRAC_INSTALL_DIR="$install_directory" \
    sh "$repository/install.sh"

cmp "$test_directory/package/vrac" "$install_directory/vrac"
test -x "$install_directory/vrac"
grep -F "/releases/latest" "$installer_log" >/dev/null
grep -F "/download/$version/$asset" "$installer_log" >/dev/null
grep -F "/download/$version/$asset.sha256" "$installer_log" >/dev/null

printf '%064d  %s\n' 0 "$asset" > "$test_directory/assets/$asset.sha256"
if PATH="$test_directory/fake-bin:$PATH" \
    VRAC_INSTALLER_TEST_ASSETS="$test_directory/assets" \
    VRAC_INSTALLER_TEST_LOG="$installer_log" \
    VRAC_INSTALL_DIR="$test_directory/rejected/bin" \
    VRAC_VERSION="$version" \
        sh "$repository/install.sh" >/dev/null 2>&1; then
    printf 'installer accepted an invalid checksum\n' >&2
    exit 1
fi
test ! -e "$test_directory/rejected/bin/vrac"
