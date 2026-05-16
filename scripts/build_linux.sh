#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# Build script for Linux x86_64
# Produces: CLAP, VST3
# Requirements: Rust stable, system dev libraries
# ═══════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ── Configuration ──────────────────────────────────────────────────────────
PLUGIN_NAME="Nebula Stereo Delay"
PACKAGE_NAME="nebula-stereo-delay"
LIB_CRATE_NAME="nebula_stereo_delay"
VERSION="1.0.0"
VENDOR="Nebula Audio"

TARGET="x86_64-unknown-linux-gnu"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_DIR="${PROJECT_ROOT}/build/linux"

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

check_lib() {
    pkg-config --exists "$1" 2>/dev/null
}

# ── Banner ─────────────────────────────────────────────────────────────────
echo -e ""
echo -e "${BOLD}╔══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║  Nebula Stereo Delay — Linux x86_64 Build                  ║${NC}"
echo -e "${BOLD}║  Version ${VERSION}  •  ${VENDOR}                        ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════════════════════════╝${NC}"
echo -e ""

# ── Step 1: Check required tools and libraries ────────────────────────────
step "Checking required tools and system libraries..."
check_tool cargo
check_tool rustc
check_tool pkg-config
success "Core tools found"

# Check for common system libraries needed by nih-plug (via baseview/egui)
MISSING_LIBS=()

if ! check_lib "xcb"; then
    MISSING_LIBS+=("libxcb-dev")
fi
if ! check_lib "x11"; then
    MISSING_LIBS+=("libx11-dev")
fi
if ! check_lib "gl"; then
    MISSING_LIBS+=("libgl-dev")
fi

if [[ ${#MISSING_LIBS[@]} -gt 0 ]]; then
    warn "Missing system libraries detected:"
    for lib in "${MISSING_LIBS[@]}"; do
        warn "  - ${lib}"
    done
    echo ""
    info "On Debian/Ubuntu, install with:"
    echo -e "  ${BOLD}sudo apt-get install ${MISSING_LIBS[*]}${NC}"
    echo ""
    info "On Fedora/RHEL, install with:"
    echo -e "  ${BOLD}sudo dnf install libxcb-devel libX11-devel mesa-libGL-devel${NC}"
    echo ""
    info "On Arch Linux, install with:"
    echo -e "  ${BOLD}sudo pacman -S libxcb libx11 mesa${NC}"
    echo ""
    die "Please install missing system libraries and re-run this script."
fi

success "All required system libraries found"

# Verify the Rust target is available
if ! rustup target list --installed 2>/dev/null | grep -q "${TARGET}"; then
    info "Adding Rust target ${TARGET}..."
    rustup target add "${TARGET}"
fi
success "Target ${TARGET} available"

# ── Step 2: Build release binary ──────────────────────────────────────────
step "Building release binary for ${TARGET}..."
cd "${PROJECT_ROOT}"
cargo build --release --target "${TARGET}"
success "Release build complete"

# ── Step 3: Verify output .so ─────────────────────────────────────────────
LIB_PATH="${PROJECT_ROOT}/target/${TARGET}/release/lib${LIB_CRATE_NAME}.so"

if [[ ! -f "${LIB_PATH}" ]]; then
    die "Expected shared library not found at: ${LIB_PATH}"
fi

LIB_SIZE=$(du -h "${LIB_PATH}" | cut -f1)
success "Shared library found: ${LIB_PATH} (${LIB_SIZE})"

# ── Step 4: Create CLAP bundle ────────────────────────────────────────────
step "Creating CLAP bundle..."

mkdir -p "${BUILD_DIR}"

# On Linux, a CLAP plugin is a directory ending in .clap containing the .so
CLAP_BUNDLE="${BUILD_DIR}/${PLUGIN_NAME}.clap"
rm -rf "${CLAP_BUNDLE}"
mkdir -p "${CLAP_BUNDLE}"

cp "${LIB_PATH}" "${CLAP_BUNDLE}/${PLUGIN_NAME}.so"

success "CLAP bundle created: ${CLAP_BUNDLE}"

# ── Step 5: Create VST3 bundle ────────────────────────────────────────────
step "Creating VST3 bundle..."

# On Linux, VST3 follows a similar structure to Windows
VST3_BUNDLE="${BUILD_DIR}/${PLUGIN_NAME}.vst3"
VST3_CONTENTS="${VST3_BUNDLE}/Contents"
VST3_ARCH_DIR="${VST3_CONTENTS}/x86_64-linux"

rm -rf "${VST3_BUNDLE}"
mkdir -p "${VST3_ARCH_DIR}"

cp "${LIB_PATH}" "${VST3_ARCH_DIR}/${PLUGIN_NAME}.so"

# Create moduleinfo.json (optional but recommended for VST3 SDK 3.7+)
cat > "${VST3_CONTENTS}/moduleinfo.json" <<EOF
{
    "Name": "${PLUGIN_NAME}",
    "Version": "${VERSION}",
    "Description": "${PLUGIN_NAME} by ${VENDOR}",
    "Vendor": "${VENDOR}",
    "SDKVersion": "3.7.9",
    "Compatibility": {
        "PlugInCategory": "Fx|Delay"
    }
}
EOF

success "VST3 bundle created: ${VST3_BUNDLE}"

# ── Step 6: Validate bundles ──────────────────────────────────────────────
step "Validating build artifacts..."

VALID=1

# Validate CLAP
if [[ -f "${CLAP_BUNDLE}/${PLUGIN_NAME}.so" ]]; then
    if file "${CLAP_BUNDLE}/${PLUGIN_NAME}.so" | grep -q "ELF 64-bit"; then
        success "CLAP bundle: valid ELF 64-bit shared object"
    else
        error "CLAP bundle: not a valid ELF shared object"
        VALID=0
    fi
else
    error "CLAP bundle: missing shared library"
    VALID=0
fi

# Validate VST3
if [[ -f "${VST3_ARCH_DIR}/${PLUGIN_NAME}.so" ]]; then
    if file "${VST3_ARCH_DIR}/${PLUGIN_NAME}.so" | grep -q "ELF 64-bit"; then
        success "VST3 bundle: valid ELF 64-bit shared object"
    else
        error "VST3 bundle: not a valid ELF shared object"
        VALID=0
    fi
else
    error "VST3 bundle: missing shared library"
    VALID=0
fi

# ── Summary ───────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}════════════════════════════════════════════════════════════════${NC}"
echo -e "${BOLD}  Build Summary — Linux x86_64${NC}"
echo -e "${BOLD}════════════════════════════════════════════════════════════════${NC}"
echo -e ""
echo -e "  Plugin:       ${BOLD}${PLUGIN_NAME}${NC}"
echo -e "  Version:      ${BOLD}${VERSION}${NC}"
echo -e "  Vendor:       ${BOLD}${VENDOR}${NC}"
echo -e "  Target:       ${BOLD}${TARGET}${NC}"
echo -e ""
echo -e "  Output directory: ${BOLD}${BUILD_DIR}${NC}"
echo -e ""
echo -e "  ${GREEN}CLAP${NC}  →  ${CLAP_BUNDLE}"
echo -e "    ${PLUGIN_NAME}.so"
echo -e ""
echo -e "  ${GREEN}VST3${NC}  →  ${VST3_BUNDLE}"
echo -e "    Contents/"
echo -e "      x86_64-linux/"
echo -e "        ${PLUGIN_NAME}.so"
echo -e "      moduleinfo.json"
echo -e ""

if [[ "${VALID}" -eq 1 ]]; then
    echo -e "  ${GREEN}${BOLD}All bundles validated successfully!${NC}"
else
    echo -e "  ${RED}${BOLD}Some bundles have issues — see errors above.${NC}"
fi

echo -e ""
echo -e "${BOLD}  Install locations:${NC}"
echo -e "    CLAP:  ~/.clap/  or  /usr/lib/clap/"
echo -e "    VST3:  ~/.vst3/  or  /usr/lib/vst3/"
echo -e ""

# ── Optional install ──────────────────────────────────────────────────────
if [[ "${1:-}" == "--install" ]]; then
    step "Installing plugins to user directories..."
    mkdir -p ~/.clap
    mkdir -p ~/.vst3

    cp -R "${CLAP_BUNDLE}" ~/.clap/
    success "CLAP installed to ~/.clap/"
    cp -R "${VST3_BUNDLE}" ~/.vst3/
    success "VST3 installed to ~/.vst3/"
fi

if [[ "${VALID}" -eq 1 ]]; then
    exit 0
else
    exit 1
fi
