#!/usr/bin/env bash
set -euo pipefail

readonly CEF_BRANCH=7977
readonly CEF_COMMIT=708dc140cbc3286826a8abef89dc23a44ff9ea72
readonly CEF_VERSION=152.0.6
readonly GPERF_VERSION=3.3
readonly SYNC_JOBS=6

repository=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
build_dir=${ATOLL_CEF_BUILD_DIR:-$HOME/.cache/atoll/cef-build}
jobs=${ATOLL_CEF_JOBS:-$(($(nproc) * 5 / 8))}
chromium_dir=$build_dir/chromium/src
output_dir=out/Release_GN_x64
distrib_dir=$build_dir/binary_distrib
install_dir=$repository/.cef/$CEF_VERSION/cef_linux_x86_64
prebuilt_dir=$repository/.cef/$CEF_VERSION/prebuilt_linux_x86_64
checkout_done=$build_dir/.checkout-done

export GN_DEFINES="is_official_build=true use_sysroot=true symbol_level=0 is_cfi=false proprietary_codecs=true ffmpeg_branding=Chrome"
export GN_OUT_CONFIGS=Release_GN_x64
export CEF_ARCHIVE_FORMAT=tar.bz2
export PATH=$build_dir/tools/bin:$build_dir/depot_tools:$PATH

usage() {
    cat <<EOF
Builds CEF $CEF_VERSION (branch $CEF_BRANCH) with the proprietary codecs H.264 and AAC and
installs it into .cef/, where the cef crate picks it up instead of the prebuilt download.

Usage: scripts/build-cef.sh [all|checkout|build|package|install|restore]

  checkout  fetch depot_tools, CEF and Chromium (about 60 GB)
  build     compile libcef (hours, about 40 GB)
  package   create the minimal binary distribution
  install   put it into .cef/, keeping the prebuilt one in $(basename "$prebuilt_dir")
  restore   switch back to the prebuilt CEF

Environment:
  ATOLL_CEF_BUILD_DIR  checkout and build directory (default: $build_dir)
  ATOLL_CEF_JOBS       parallel compile jobs (default: $jobs)
EOF
}

step() {
    printf '\n==> %s\n' "$*"
}

ensure_gperf() {
    if command -v gperf >/dev/null; then
        return
    fi
    step "Building gperf $GPERF_VERSION"
    local source=$build_dir/tools/src
    mkdir -p "$source"
    curl -fsSL "https://ftp.gnu.org/gnu/gperf/gperf-$GPERF_VERSION.tar.gz" | tar xz -C "$source"
    (cd "$source/gperf-$GPERF_VERSION" && ./configure --prefix="$build_dir/tools" && make && make install)
}

throttle_gclient() {
    mkdir -p "$build_dir/tools/bin"
    cat >"$build_dir/tools/bin/gclient" <<EOF
#!/usr/bin/env bash
if [[ \${1:-} != sync ]]; then
    exec "$build_dir/depot_tools/gclient" "\$@"
fi
for attempt in 1 2 3 4 5; do
    "$build_dir/depot_tools/gclient" "\$@" --jobs=$SYNC_JOBS && exit 0
    echo "gclient sync failed (attempt \$attempt of 5), retrying in two minutes" >&2
    sleep 120
done
exit 1
EOF
    chmod +x "$build_dir/tools/bin/gclient"
}

checkout() {
    mkdir -p "$build_dir"
    ensure_gperf
    throttle_gclient
    if [[ -d $build_dir/chromium && ! -e $checkout_done ]]; then
        step "Removing the unfinished Chromium checkout"
        rm -rf "$build_dir/chromium"
    fi
    step "Fetching CEF $CEF_VERSION and Chromium"
    curl -fsSL -o "$build_dir/automate-git.py" \
        "https://raw.githubusercontent.com/chromiumembedded/cef/$CEF_COMMIT/tools/automate/automate-git.py"
    python3 "$build_dir/automate-git.py" \
        --download-dir="$build_dir" \
        --depot-tools-dir="$build_dir/depot_tools" \
        --branch="$CEF_BRANCH" \
        --checkout="$CEF_COMMIT" \
        --no-chromium-history \
        --with-pgo-profiles \
        --x64-build \
        --no-debug-build \
        --no-build \
        --no-distrib
    touch "$checkout_done"
}

build() {
    ensure_gperf
    step "Generating the build files"
    (cd "$chromium_dir/cef" && python3 tools/gclient_hook.py)
    step "Compiling with $jobs jobs"
    (cd "$chromium_dir" && nice -n 19 ionice -c 3 autoninja -j "$jobs" -C "$output_dir" cefsimple chrome_sandbox)
}

package() {
    step "Creating the binary distribution"
    rm -rf "$distrib_dir"
    (cd "$chromium_dir/cef/tools" && python3 make_distrib.py \
        --output-dir="$distrib_dir" \
        --ninja-build \
        --x64-build \
        --minimal \
        --allow-partial \
        --no-symbols \
        --no-docs)
}

install_distribution() {
    local archive
    archive=$(find "$distrib_dir" -maxdepth 1 -name "cef_binary_${CEF_VERSION}+*_linux64_minimal.tar.bz2" | head -n 1)
    if [[ -z $archive ]]; then
        echo "No distribution found in $distrib_dir, run the package step first." >&2
        exit 1
    fi
    local extracted=${archive%.tar.bz2}
    step "Installing $(basename "$extracted")"
    if [[ -d $install_dir && ! -d $prebuilt_dir ]]; then
        mv "$install_dir" "$prebuilt_dir"
    fi
    rm -rf "$install_dir"
    mkdir -p "$install_dir"
    cp -a "$extracted/Release/." "$extracted/Resources/." "$install_dir/"
    cp -a "$extracted/CMakeLists.txt" "$extracted/cmake" "$extracted/include" \
        "$extracted/libcef_dll" "$extracted/CREDITS.html" "$install_dir/"
    cat >"$install_dir/archive.json" <<EOF
{
  "type": "minimal",
  "name": "$(basename "$archive")",
  "sha1": "$(sha1sum "$archive" | cut -d ' ' -f 1)"
}
EOF
    echo "Rebuild Atoll to use it, the cef crate copies the runtime files into target/."
}

restore() {
    if [[ ! -d $prebuilt_dir ]]; then
        echo "The prebuilt CEF is still in place." >&2
        exit 1
    fi
    rm -rf "$install_dir"
    mv "$prebuilt_dir" "$install_dir"
    echo "Switched back to the prebuilt CEF."
}

case ${1:-all} in
    all) checkout && build && package && install_distribution ;;
    checkout | build | package | restore) "$1" ;;
    install) install_distribution ;;
    -h | --help | help) usage ;;
    *) usage >&2 && exit 1 ;;
esac
