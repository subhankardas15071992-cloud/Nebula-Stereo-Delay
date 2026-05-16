#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# Build script for macOS — Universal Binary (arm64 + x86_64)
# Produces: CLAP, VST3, AUv2
# Requirements: Xcode Command Line Tools, Rust with arm64 and x86_64 targets
# ═══════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ── Configuration ──────────────────────────────────────────────────────────
PLUGIN_NAME="Nebula Stereo Delay"
PACKAGE_NAME="nebula-stereo-delay"
LIB_CRATE_NAME="nebula_stereo_delay"
VERSION="1.0.0"
BUNDLE_ID="audio.nebula.NebulaStereoDelay"
VENDOR="Nebula Audio"

TARGET_ARM64="aarch64-apple-darwin"
TARGET_X86="x86_64-apple-darwin"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_DIR="${PROJECT_ROOT}/build/macos"

CLAP_WRAPPER_DIR="${PROJECT_ROOT}/.clap-wrapper"
CLAP_WRAPPER_REPO="https://github.com/free-audio/clap-wrapper.git"

# ── Colors ─────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# ── Helper Functions ───────────────────────────────────────────────────────
info()    { echo -e "${BLUE}[INFO]${NC}  $*"; }
success() { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC}  $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }
step()    { echo -e "${CYAN}${BOLD}==>${NC} ${BOLD}$*${NC}"; }

die() {
    error "$@"
    exit 1
}

check_tool() {
    if ! command -v "$1" &>/dev/null; then
        die "Required tool '$1' not found. Please install it before continuing."
    fi
}

# ── Banner ─────────────────────────────────────────────────────────────────
echo -e ""
echo -e "${BOLD}╔══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║  Nebula Stereo Delay — macOS Universal Build               ║${NC}"
echo -e "${BOLD}║  Version ${VERSION}  •  ${VENDOR}                        ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════════════════════════╝${NC}"
echo -e ""

# ── Step 1: Check required tools ──────────────────────────────────────────
step "Checking required tools..."
check_tool rustup
check_tool cargo
check_tool clang
check_tool lipo
success "All required tools found"

# ── Step 2: Add Rust targets ──────────────────────────────────────────────
step "Ensuring Rust targets are installed..."
rustup target add "${TARGET_ARM64}" 2>/dev/null || true
rustup target add "${TARGET_X86}" 2>/dev/null || true
success "Targets ${TARGET_ARM64} and ${TARGET_X86} available"

# ── Step 3: Build for arm64 ───────────────────────────────────────────────
step "Building for arm64 (${TARGET_ARM64})..."
cd "${PROJECT_ROOT}"
cargo build --release --target "${TARGET_ARM64}"
success "arm64 build complete"

# ── Step 4: Build for x86_64 ──────────────────────────────────────────────
step "Building for x86_64 (${TARGET_X86})..."
cargo build --release --target "${TARGET_X86}"
success "x86_64 build complete"

# ── Step 5: Create universal binary with lipo ─────────────────────────────
step "Creating universal binary..."
mkdir -p "${BUILD_DIR}"

LIB_ARM64="${PROJECT_ROOT}/target/${TARGET_ARM64}/release/lib${LIB_CRATE_NAME}.dylib"
LIB_X86="${PROJECT_ROOT}/target/${TARGET_X86}/release/lib${LIB_CRATE_NAME}.dylib"
UNIVERSAL_LIB="${BUILD_DIR}/lib${LIB_CRATE_NAME}_universal.dylib"

if [[ ! -f "${LIB_ARM64}" ]]; then
    die "arm64 dylib not found at ${LIB_ARM64}"
fi
if [[ ! -f "${LIB_X86}" ]]; then
    die "x86_64 dylib not found at ${LIB_X86}"
fi

lipo -create "${LIB_ARM64}" "${LIB_X86}" -output "${UNIVERSAL_LIB}"
success "Universal binary created ($(du -h "${UNIVERSAL_LIB}" | cut -f1))"

# ── Step 6: Create CLAP bundle ────────────────────────────────────────────
step "Creating CLAP bundle..."

CLAP_BUNDLE="${BUILD_DIR}/${PLUGIN_NAME}.clap"
mkdir -p "${CLAP_BUNDLE}/Contents/MacOS"

cp "${UNIVERSAL_LIB}" "${CLAP_BUNDLE}/Contents/MacOS/${PLUGIN_NAME}"

cat > "${CLAP_BUNDLE}/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>${PLUGIN_NAME}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}.clap</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleExecutable</key>
    <string>${PLUGIN_NAME}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
</dict>
</plist>
EOF

success "CLAP bundle created: ${CLAP_BUNDLE}"

# ── Step 7: Create VST3 bundle ────────────────────────────────────────────
step "Creating VST3 bundle..."

VST3_BUNDLE="${BUILD_DIR}/${PLUGIN_NAME}.vst3"
mkdir -p "${VST3_BUNDLE}/Contents/MacOS"

cp "${UNIVERSAL_LIB}" "${VST3_BUNDLE}/Contents/MacOS/${PLUGIN_NAME}"

cat > "${VST3_BUNDLE}/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>${PLUGIN_NAME}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}.vst3</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
    <key>CFBundleExecutable</key>
    <string>${PLUGIN_NAME}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
</dict>
</plist>
EOF

success "VST3 bundle created: ${VST3_BUNDLE}"

# ── Step 8: Build AUv2 via clap-wrapper ────────────────────────────────────
step "Building AUv2 component via clap-wrapper..."

# Clone clap-wrapper if not present
if [[ ! -d "${CLAP_WRAPPER_DIR}/.git" ]]; then
    info "Cloning clap-wrapper repository..."
    git clone --depth 1 "${CLAP_WRAPPER_REPO}" "${CLAP_WRAPPER_DIR}"
else
    info "clap-wrapper already cloned, pulling latest..."
    (cd "${CLAP_WRAPPER_DIR}" && git pull --ff-only 2>/dev/null || warn "Could not update clap-wrapper, using existing version")
fi

# Build the AUv2 wrapper
AU2_BUILD_DIR="${CLAP_WRAPPER_DIR}/build-au2"
mkdir -p "${AU2_BUILD_DIR}"

# Configure and build clap-wrapper for AUv2
info "Configuring clap-wrapper for AUv2..."
cd "${AU2_BUILD_DIR}"

cmake .. \
    -DCLAP_WRAPPER_BUILD_AUV2=ON \
    -DCLAP_WRAPPER_BUILD_VST3=OFF \
    -DCLAP_WRAPPER_BUILD_STANDALONE=OFF \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_OSX_ARCHITECTURES="arm64;x86_64" \
    -DCLAP_WRAPPER_PLUGIN_NAME="${PLUGIN_NAME}" \
    -DCLAP_WRAPPER_PLUGIN_VERSION="${VERSION}" \
    -DCLAP_WRAPPER_BUNDLE_ID="${BUNDLE_ID}" \
    -DCLAP_WRAPPER_CLAP_PATH="${CLAP_BUNDLE}" \
    2>/dev/null || {
        warn "clap-wrapper cmake configuration failed — attempting manual AUv2 stub build"
        AU2_BUNDLE="${BUILD_DIR}/${PLUGIN_NAME}.component"
        mkdir -p "${AU2_BUNDLE}/Contents/MacOS"
        cp "${UNIVERSAL_LIB}" "${AU2_BUNDLE}/Contents/MacOS/${PLUGIN_NAME}"

        cat > "${AU2_BUNDLE}/Contents/Info.plist" <<AU2EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>${PLUGIN_NAME}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}.auv2</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
    <key>CFBundleExecutable</key>
    <string>${PLUGIN_NAME}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>AudioComponents</key>
    <array>
        <dict>
            <key>name</key>
            <string>${VENDOR}: ${PLUGIN_NAME}</string>
            <key>description</key>
            <string>${PLUGIN_NAME} by ${VENDOR}</string>
            <key>factoryFunction</key>
            <string>${BUNDLE_ID}_Factory</string>
            <key>manufacturer</key>
            <string>NebA</string>
            <key>type</key>
            <string>aufx</string>
            <key>subtype</key>
            <string>NSDl</string>
            <key>version</key>
            <integer>65536</integer>
            <key>sandboxSafe</key>
            <true/>
        </dict>
    </array>
</dict>
</plist>
AU2EOF

        warn "AUv2 bundle created with raw dylib — clap-wrapper cmake failed."
        warn "You may need to build the clap-wrapper AUv2 adapter manually."
        warn "See: https://github.com/free-audio/clap-wrapper"
        success "AUv2 stub bundle created: ${AU2_BUNDLE}"
        cd "${PROJECT_ROOT}"
        SKIP_AU2_FINAL=1
}

if [[ "${SKIP_AU2_FINAL:-0}" != "1" ]]; then
    info "Building AUv2 wrapper..."
    cmake --build . --config Release -j"$(sysctl -n hw.ncpu 2>/dev/null || echo 4)"

    # Locate the built AUv2 component from the clap-wrapper output
    FOUND_COMPONENT=""
    for search_path in \
        "${AU2_BUILD_DIR}/${PLUGIN_NAME}.component" \
        "${AU2_BUILD_DIR}/Release/${PLUGIN_NAME}.component" \
        "${CLAP_WRAPPER_DIR}/build/${PLUGIN_NAME}.component"; do
        if [[ -d "${search_path}" ]]; then
            FOUND_COMPONENT="${search_path}"
            break
        fi
    done

    AU2_BUNDLE="${BUILD_DIR}/${PLUGIN_NAME}.component"

    if [[ -n "${FOUND_COMPONENT}" && -d "${FOUND_COMPONENT}" ]]; then
        cp -R "${FOUND_COMPONENT}" "${AU2_BUNDLE}"
        success "AUv2 component built and copied: ${AU2_BUNDLE}"
    else
        warn "Could not locate built AUv2 component in clap-wrapper output"
        warn "Creating AUv2 bundle with universal dylib as fallback"

        mkdir -p "${AU2_BUNDLE}/Contents/MacOS"
        cp "${UNIVERSAL_LIB}" "${AU2_BUNDLE}/Contents/MacOS/${PLUGIN_NAME}"

        cat > "${AU2_BUNDLE}/Contents/Info.plist" <<AU2EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>${PLUGIN_NAME}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}.auv2</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
    <key>CFBundleExecutable</key>
    <string>${PLUGIN_NAME}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>AudioComponents</key>
    <array>
        <dict>
            <key>name</key>
            <string>${VENDOR}: ${PLUGIN_NAME}</string>
            <key>description</key>
            <string>${PLUGIN_NAME} by ${VENDOR}</string>
            <key>factoryFunction</key>
            <string>${BUNDLE_ID}_Factory</string>
            <key>manufacturer</key>
            <string>NebA</string>
            <key>type</key>
            <string>aufx</string>
            <key>subtype</key>
            <string>NSDl</string>
            <key>version</key>
            <integer>65536</integer>
            <key>sandboxSafe</key>
            <true/>
        </dict>
    </array>
</dict>
</plist>
AU2EOF

        warn "AUv2 bundle created as fallback — for production use, build clap-wrapper properly"
        success "AUv2 fallback bundle created: ${AU2_BUNDLE}"
    fi

    cd "${PROJECT_ROOT}"
fi

# ── Step 9: Validate bundles ──────────────────────────────────────────────
step "Validating build artifacts..."

VALID=1

for bundle in "${CLAP_BUNDLE}" "${VST3_BUNDLE}" "${AU2_BUNDLE}"; do
    bundle_type="${bundle##*.}"
    bundle_label="$(printf '%s' "${bundle_type}" | tr '[:lower:]' '[:upper:]')"
    if [[ -d "${bundle}" ]]; then
        if [[ -f "${bundle}/Contents/MacOS/${PLUGIN_NAME}" ]]; then
            if file "${bundle}/Contents/MacOS/${PLUGIN_NAME}" | grep -q "Mach-O"; then
                if file "${bundle}/Contents/MacOS/${PLUGIN_NAME}" | grep -q "universal"; then
                    success "${bundle_label} bundle: valid (universal binary)"
                else
                    warn "${bundle_label} bundle: valid (single architecture)"
                fi
            else
                error "${bundle_label} bundle: executable is not a valid Mach-O binary"
                VALID=0
            fi
        else
            error "${bundle_label} bundle: missing executable"
            VALID=0
        fi

        if [[ ! -f "${bundle}/Contents/Info.plist" ]]; then
            error "${bundle_label} bundle: missing Info.plist"
            VALID=0
        fi
    else
        error "${bundle_label} bundle: directory not found"
        VALID=0
    fi
done

# ── Step 10: Print summary ───────────────────────────────────────────────
echo ""
echo -e "${BOLD}════════════════════════════════════════════════════════════════${NC}"
echo -e "${BOLD}  Build Summary — macOS Universal Binary${NC}"
echo -e "${BOLD}════════════════════════════════════════════════════════════════${NC}"
echo -e ""
echo -e "  Plugin:       ${BOLD}${PLUGIN_NAME}${NC}"
echo -e "  Version:      ${BOLD}${VERSION}${NC}"
echo -e "  Vendor:       ${BOLD}${VENDOR}${NC}"
echo -e "  Bundle ID:    ${BOLD}${BUNDLE_ID}${NC}"
echo -e "  Targets:      ${BOLD}${TARGET_ARM64} + ${TARGET_X86}${NC}"
echo -e ""
echo -e "  Output directory: ${BOLD}${BUILD_DIR}${NC}"
echo -e ""
echo -e "  ${GREEN}CLAP${NC}  →  ${CLAP_BUNDLE}"
echo -e "  ${GREEN}VST3${NC}  →  ${VST3_BUNDLE}"
echo -e "  ${GREEN}AUv2${NC}  →  ${AU2_BUNDLE}"
echo -e ""

if [[ "${VALID}" -eq 1 ]]; then
    echo -e "  ${GREEN}${BOLD}All bundles validated successfully!${NC}"
else
    echo -e "  ${RED}${BOLD}Some bundles have issues — see errors above.${NC}"
fi

echo -e ""
echo -e "${BOLD}  Install locations:${NC}"
echo -e "    CLAP:  ~/Library/Audio/Plug-Ins/CLAP/"
echo -e "    VST3:  ~/Library/Audio/Plug-Ins/VST3/"
echo -e "    AUv2:  ~/Library/Audio/Plug-Ins/Components/"
echo -e ""

# Copy to install locations if --install flag is given
if [[ "${1:-}" == "--install" ]]; then
    step "Installing plugins to user Library..."
    mkdir -p ~/Library/Audio/Plug-Ins/CLAP
    mkdir -p ~/Library/Audio/Plug-Ins/VST3
    mkdir -p ~/Library/Audio/Plug-Ins/Components

    cp -R "${CLAP_BUNDLE}" ~/Library/Audio/Plug-Ins/CLAP/
    success "CLAP installed"
    cp -R "${VST3_BUNDLE}" ~/Library/Audio/Plug-Ins/VST3/
    success "VST3 installed"
    cp -R "${AU2_BUNDLE}" ~/Library/Audio/Plug-Ins/Components/
    success "AUv2 installed"

    # Clear AU cache so macOS picks up the new component
    auval -cache 2>/dev/null || true
    info "Run 'auval -a' to verify the AUv2 component is visible to macOS"
fi

if [[ "${VALID}" -eq 1 ]]; then
    exit 0
else
    exit 1
fi
