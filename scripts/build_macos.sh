#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# Build script for macOS — Universal Binary (arm64 + x86_64)
# Produces: CLAP, VST3, AUv2
# Requirements: Xcode Command Line Tools, Git, Rust with arm64 and x86_64 targets
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
check_tool clang++
check_tool git
check_tool lipo
check_tool plutil
check_tool codesign
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
rm -rf "${CLAP_BUNDLE}"
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
    <key>CFBundleExecutable</key>
    <string>${PLUGIN_NAME}</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleSupportedPlatforms</key>
    <array>
        <string>MacOSX</string>
    </array>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF
echo "BNDL????" > "${CLAP_BUNDLE}/Contents/PkgInfo"

success "CLAP bundle created: ${CLAP_BUNDLE}"

# ── Step 7: Create VST3 bundle ────────────────────────────────────────────
step "Creating VST3 bundle..."

VST3_BUNDLE="${BUILD_DIR}/${PLUGIN_NAME}.vst3"
rm -rf "${VST3_BUNDLE}"
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
    <key>CFBundleSupportedPlatforms</key>
    <array>
        <string>MacOSX</string>
    </array>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF
echo "BNDL????" > "${VST3_BUNDLE}/Contents/PkgInfo"

success "VST3 bundle created: ${VST3_BUNDLE}"

# ── Step 8: Create AUv2 component with clap-wrapper-rs ────────────────────
step "Creating AUv2 component through clap-wrapper-rs..."

AUV2_BUNDLE="${BUILD_DIR}/${PLUGIN_NAME}.component"
AUV2_BUNDLE_VERSION="1.0.1"
AUV2_VERSION=65537

rm -rf "${AUV2_BUNDLE}"
mkdir -p "${AUV2_BUNDLE}/Contents/MacOS"

cp "${UNIVERSAL_LIB}" "${AUV2_BUNDLE}/Contents/MacOS/${PLUGIN_NAME}"

cat > "${AUV2_BUNDLE}/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>AudioComponents</key>
    <array>
        <dict>
            <key>description</key>
            <string>Professional stereo delay audio effect</string>
            <key>factoryFunction</key>
            <string>GetPluginFactoryAUV2_0</string>
            <key>manufacturer</key>
            <string>NbAu</string>
            <key>name</key>
            <string>${VENDOR}: ${PLUGIN_NAME}</string>
            <key>resourceUsage</key>
            <dict>
                <key>temporary-exception.files.all.read-write</key>
                <true/>
            </dict>
            <key>subtype</key>
            <string>NsDl</string>
            <key>type</key>
            <string>aufx</string>
            <key>version</key>
            <integer>${AUV2_VERSION}</integer>
        </dict>
    </array>
    <key>CFBundleDevelopmentRegion</key>
    <string>English</string>
    <key>CFBundleExecutable</key>
    <string>${PLUGIN_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${PLUGIN_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}.auv2</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>${PLUGIN_NAME}</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>CFBundleShortVersionString</key>
    <string>${AUV2_BUNDLE_VERSION}</string>
    <key>CFBundleSupportedPlatforms</key>
    <array>
        <string>MacOSX</string>
    </array>
    <key>CFBundleVersion</key>
    <string>${AUV2_BUNDLE_VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.13</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string></string>
</dict>
</plist>
EOF
echo "BNDL????" > "${AUV2_BUNDLE}/Contents/PkgInfo"

if [[ ! -d "${AUV2_BUNDLE}" ]]; then
    die "AUv2 component was not created at ${AUV2_BUNDLE}"
fi

success "AUv2 component created: ${AUV2_BUNDLE}"

# ── Step 9: Ad-hoc sign bundles ───────────────────────────────────────────
step "Ad-hoc signing macOS bundles..."

for bundle in "${CLAP_BUNDLE}" "${VST3_BUNDLE}" "${AUV2_BUNDLE}"; do
    codesign --force --deep --sign - --timestamp=none "${bundle}" >/dev/null
    success "Signed ${bundle##*/}"
done

# ── Step 10: Validate bundles ─────────────────────────────────────────────
step "Validating build artifacts..."

VALID=1

for bundle in "${CLAP_BUNDLE}" "${VST3_BUNDLE}" "${AUV2_BUNDLE}"; do
    bundle_type="${bundle##*.}"
    bundle_label="$(printf '%s' "${bundle_type}" | tr '[:lower:]' '[:upper:]')"
    executable_path="${bundle}/Contents/MacOS/${PLUGIN_NAME}"
    if [[ -d "${bundle}" ]]; then
        if [[ -f "${executable_path}" ]]; then
            if file "${executable_path}" | grep -q "Mach-O"; then
                if file "${executable_path}" | grep -q "universal"; then
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
        elif ! plutil -lint "${bundle}/Contents/Info.plist" >/dev/null; then
            error "${bundle_label} bundle: Info.plist failed plutil validation"
            VALID=0
        fi

        if ! codesign --verify --deep --strict "${bundle}" >/dev/null 2>&1; then
            error "${bundle_label} bundle: codesign verification failed"
            VALID=0
        fi
    else
        error "${bundle_label} bundle: directory not found"
        VALID=0
    fi
done

run_auv2_validation() {
    if ! command -v auval >/dev/null; then
        warn "auval not found; skipping AUv2 host validation"
        return 0
    fi

    local component_dir="${HOME}/Library/Audio/Plug-Ins/Components"
    local install_bundle="${component_dir}/${PLUGIN_NAME}.component"
    local backup_dir=""
    local backup_bundle=""
    local auval_log="${BUILD_DIR}/auval.log"
    local status=1

    mkdir -p "${component_dir}"

    if [[ -e "${install_bundle}" ]]; then
        backup_dir="$(mktemp -d)"
        backup_bundle="${backup_dir}/${PLUGIN_NAME}.component"
        mv "${install_bundle}" "${backup_bundle}"
    fi

    if cp -R "${AUV2_BUNDLE}" "${install_bundle}"; then
        for attempt in {1..10}; do
            killall -9 AudioComponentRegistrar >/dev/null 2>&1 || true
            sleep 3
            auval -a >/dev/null 2>&1 || true

            if auval -v aufx NsDl NbAu >"${auval_log}" 2>&1; then
                status=0
                break
            fi
        done

        if [[ "${status}" -ne 0 && -f "${auval_log}" ]]; then
            cat "${auval_log}"
        fi
    fi

    rm -rf "${install_bundle}"

    if [[ -n "${backup_bundle}" && -e "${backup_bundle}" ]]; then
        mv "${backup_bundle}" "${install_bundle}"
        rm -rf "${backup_dir}"
    fi

    killall -9 AudioComponentRegistrar >/dev/null 2>&1 || true
    return "${status}"
}

if [[ "${RUN_AUVAL:-0}" == "1" ]]; then
    if run_auv2_validation; then
        success "AUv2 component: auval validation passed"
    else
        error "AUv2 component: auval validation failed"
        VALID=0
    fi
else
    warn "Skipping AUv2 auval validation; set RUN_AUVAL=1 to install temporarily and validate"
fi

# ── Step 11: Print summary ───────────────────────────────────────────────
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
echo -e "  ${GREEN}AUv2${NC}  →  ${AUV2_BUNDLE}"
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
    cp -R "${AUV2_BUNDLE}" ~/Library/Audio/Plug-Ins/Components/
    success "AUv2 installed"
fi

if [[ "${VALID}" -eq 1 ]]; then
    exit 0
else
    exit 1
fi
