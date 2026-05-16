#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# Clean script — removes all build artifacts for Nebula Stereo Delay
# ═══════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ── Configuration ──────────────────────────────────────────────────────────
PLUGIN_NAME="Nebula Stereo Delay"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# ── Colors ─────────────────────────────────────────────────────────────────
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ── Helper Functions ───────────────────────────────────────────────────────
info()    { echo -e "${BLUE}[INFO]${NC}  $*"; }
success() { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC}  $*"; }
step()    { echo -e "${CYAN}${BOLD}==>${NC} ${BOLD}$*${NC}"; }

# ── Banner ─────────────────────────────────────────────────────────────────
echo -e ""
echo -e "${BOLD}  Cleaning build artifacts for ${PLUGIN_NAME}${NC}"
echo -e ""

# ── Remove Cargo build artifacts ──────────────────────────────────────────
step "Cleaning Cargo target directory..."
if [[ -d "${PROJECT_ROOT}/target" ]]; then
    cargo clean --manifest-path "${PROJECT_ROOT}/Cargo.toml" 2>/dev/null && \
        success "Cargo target directory cleaned" || \
        warn "Could not fully clean Cargo target directory"
else
    info "No target directory found — skipping"
fi

# ── Remove platform build output directories ──────────────────────────────
step "Removing platform build directories..."

for platform_dir in macos windows linux; do
    dir="${PROJECT_ROOT}/build/${platform_dir}"
    if [[ -d "${dir}" ]]; then
        rm -rf "${dir}"
        success "Removed build/${platform_dir}/"
    fi
done

# Remove the top-level build directory if empty
if [[ -d "${PROJECT_ROOT}/build" ]]; then
    rmdir "${PROJECT_ROOT}/build" 2>/dev/null && \
        success "Removed empty build/ directory" || true
fi

# ── Remove any leftover artifacts in project root ─────────────────────────
step "Cleaning miscellaneous artifacts..."
for pattern in "libnebula_stereo_delay_universal.dylib" "*.clap" "*.vst3"; do
    for file in "${PROJECT_ROOT}"/${pattern}; do
        if [[ -e "${file}" ]]; then
            rm -rf "${file}"
            success "Removed $(basename "${file}")"
        fi
    done
done

# ── Summary ───────────────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}${BOLD}Clean complete!${NC} All build artifacts have been removed."
echo ""
