#!/bin/sh

set -eu

repository="yan-imensar/vrac"
release_root="https://github.com/$repository/releases"
temporary_directory=""
staged_binary=""

fail() {
    printf 'vrac installer: %s\n' "$1" >&2
    exit 1
}

cleanup() {
    if [ -n "$staged_binary" ]; then
        rm -f "$staged_binary"
    fi
    if [ -n "$temporary_directory" ]; then
        rm -rf "$temporary_directory"
    fi
}

download() {
    curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
        --retry 3 --output "$2" "$1"
}

[ "$#" -eq 0 ] || fail "this installer does not accept arguments"
command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"
[ -n "${HOME:-}" ] || fail "HOME is not set"

case "$(uname -s):$(uname -m)" in
    Darwin:arm64)
        target="aarch64-apple-darwin"
        ;;
    Darwin:x86_64)
        target="x86_64-apple-darwin"
        ;;
    Linux:x86_64 | Linux:amd64)
        target="x86_64-unknown-linux-musl"
        ;;
    *)
        fail "unsupported platform: $(uname -s) $(uname -m)"
        ;;
esac

version="${VRAC_VERSION:-}"
if [ -z "$version" ]; then
    latest_url=$(curl --proto '=https' --tlsv1.2 --fail --location --silent \
        --show-error --output /dev/null --write-out '%{url_effective}' \
        "$release_root/latest")
    version=${latest_url##*/}
fi

case "$version" in
    v[0-9]*) ;;
    *) fail "invalid release version: $version" ;;
esac
case "$version" in
    *[!A-Za-z0-9._-]*) fail "invalid release version: $version" ;;
esac

asset="vrac-$version-$target.tar.gz"
asset_url="$release_root/download/$version/$asset"
install_directory="${VRAC_INSTALL_DIR:-$HOME/.local/bin}"

temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/vrac-installer.XXXXXX") \
    || fail "could not create a temporary directory"
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

cd "$temporary_directory"
printf 'Downloading Vrac %s for %s...\n' "$version" "$target"
download "$asset_url" "$asset"
download "$asset_url.sha256" "$asset.sha256"

if command -v sha256sum >/dev/null 2>&1; then
    sha256sum --check "$asset.sha256" >/dev/null \
        || fail "the release checksum does not match"
elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 --check "$asset.sha256" >/dev/null \
        || fail "the release checksum does not match"
else
    fail "sha256sum or shasum is required to verify the download"
fi

tar -xzf "$asset"
[ -f vrac ] || fail "the release archive does not contain vrac"
[ -f LICENSE ] || fail "the release archive does not contain LICENSE"
chmod 755 vrac

installed_version=$(./vrac --version) \
    || fail "the downloaded binary could not be executed"
[ "$installed_version" = "vrac ${version#v}" ] \
    || fail "the downloaded binary reports an unexpected version"

mkdir -p "$install_directory"
staged_binary="$install_directory/.vrac.$$"
cp vrac "$staged_binary"
chmod 755 "$staged_binary"
mv -f "$staged_binary" "$install_directory/vrac"
staged_binary=""

printf 'Installed Vrac %s to %s/vrac\n' "$version" "$install_directory"
case ":${PATH:-}:" in
    *":$install_directory:"*) ;;
    *)
        printf 'Add %s to PATH before running vrac.\n' "$install_directory"
        ;;
esac
