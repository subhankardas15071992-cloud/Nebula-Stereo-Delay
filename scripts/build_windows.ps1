# ═══════════════════════════════════════════════════════════════════════════
# Build script for Windows x86_64
# Produces: VST3
# Requirements: Rust stable, MSVC build tools
# ═══════════════════════════════════════════════════════════════════════════

# ── Configuration ──────────────────────────────────────────────────────────
$PluginName     = "Nebula Stereo Delay"
$PackageName    = "nebula-stereo-delay"
$LibCrateName   = "nebula_stereo_delay"
$Version        = "1.0.0"
$Vendor         = "Nebula Audio"

$Target         = "x86_64-pc-windows-msvc"

$ScriptDir      = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot    = Resolve-Path (Join-Path $ScriptDir "..")
$BuildDir       = Join-Path $ProjectRoot "build\windows"

# ── Helper Functions ───────────────────────────────────────────────────────
function Write-Step    { param([string]$Msg) Write-Host "`n==>" -ForegroundColor Cyan -NoNewline; Write-Host " $Msg" -ForegroundColor White }
function Write-Info    { param([string]$Msg) Write-Host "[INFO]  " -ForegroundColor Blue -NoNewline; Write-Host $Msg }
function Write-Ok      { param([string]$Msg) Write-Host "[OK]    " -ForegroundColor Green -NoNewline; Write-Host $Msg }
function Write-WarnMsg { param([string]$Msg) Write-Host "[WARN]  " -ForegroundColor Yellow -NoNewline; Write-Host $Msg }
function Write-ErrMsg  { param([string]$Msg) Write-Host "[ERROR] " -ForegroundColor Red -NoNewline; Write-Host $Msg }

function Test-Command {
    param([string]$Name)
    return [bool](Get-Command -Name $Name -ErrorAction SilentlyContinue)
}

# ── Banner ─────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "================================================================" -ForegroundColor White
Write-Host "  Nebula Stereo Delay - Windows x86_64 Build" -ForegroundColor White
Write-Host "  Version $Version  -  $Vendor" -ForegroundColor White
Write-Host "================================================================" -ForegroundColor White
Write-Host ""

# ── Step 1: Check for cargo ───────────────────────────────────────────────
Write-Step "Checking required tools..."

if (-not (Test-Command "cargo")) {
    Write-ErrMsg "Required tool 'cargo' not found. Install Rust from https://rustup.rs/"
    exit 1
}
Write-Ok "cargo found"

if (-not (Test-Command "rustc")) {
    Write-ErrMsg "Required tool 'rustc' not found."
    exit 1
}
Write-Ok "rustc found"

if (-not (Test-Command "rustup")) {
    Write-ErrMsg "Required tool 'rustup' not found."
    exit 1
}
Write-Ok "rustup found"

# Verify MSVC target is available
$targetList = rustup target list --installed 2>$null
if ($targetList -notcontains $Target) {
    Write-Info "Adding Rust target $Target..."
    rustup target add $Target
    if ($LASTEXITCODE -ne 0) {
        Write-ErrMsg "Failed to add target $Target"
        exit 1
    }
}
Write-Ok "Target $Target available"

# ── Step 2: Build release binary ──────────────────────────────────────────
Write-Step "Building release binary for $Target..."

Push-Location $ProjectRoot
try {
    cargo build --release --target $Target --no-default-features --features plugin,gui
    if ($LASTEXITCODE -ne 0) {
        Write-ErrMsg "Build failed with exit code $LASTEXITCODE"
        Pop-Location
        exit 1
    }
} catch {
    Write-ErrMsg "Build failed: $_"
    Pop-Location
    exit 1
}
Pop-Location

Write-Ok "Release build complete"

# ── Step 3: Verify output DLL ─────────────────────────────────────────────
$DllPath = Join-Path $ProjectRoot "target\$Target\release\$LibCrateName.dll"

if (-not (Test-Path $DllPath)) {
    Write-ErrMsg "Expected DLL not found at: $DllPath"
    exit 1
}

$DllSize = (Get-Item $DllPath).Length / 1MB
Write-Ok "DLL found: $DllPath ($($DllSize.ToString('F1')) MB)"

# ── Step 4: Create VST3 bundle directory structure ─────────────────────────
Write-Step "Creating VST3 bundle..."

$Vst3Bundle = Join-Path $BuildDir "$PluginName.vst3"
$Vst3Contents = Join-Path $Vst3Bundle "Contents"
$Vst3ArchDir  = Join-Path $Vst3Contents "x86_64-win"

# Clean previous build
if (Test-Path $Vst3Bundle) {
    Remove-Item -Recurse -Force $Vst3Bundle
}

New-Item -ItemType Directory -Path $Vst3ArchDir -Force | Out-Null

# ── Step 5: Copy DLL to proper location ───────────────────────────────────
# On Windows, VST3 bundles use a .vst3 extension for the DLL inside
# the platform-specific directory
$Vst3Binary = Join-Path $Vst3ArchDir "$PluginName.vst3"

Copy-Item -Path $DllPath -Destination $Vst3Binary -Force
Write-Ok "DLL copied to VST3 bundle"

# Create a minimal moduleinfo.json (optional but recommended for VST3 SDK 3.7+)
$ModuleInfo = @"
{
    "Name": "$PluginName",
    "Version": "$Version",
    "Description": "$PluginName by $Vendor",
    "Vendor": "$Vendor",
    "SDKVersion": "3.7.9",
    "Compatibility": {
        "PlugInCategory": "Fx|Delay"
    }
}
"@

Set-Content -Path (Join-Path $Vst3Contents "moduleinfo.json") -Value $ModuleInfo -Encoding UTF8
Write-Ok "moduleinfo.json created"

# ── Step 6: Validate ──────────────────────────────────────────────────────
Write-Step "Validating build artifacts..."

$Valid = $true

if (Test-Path $Vst3Binary) {
    $binaryInfo = [System.IO.FileInfo]::new($Vst3Binary)
    if ($binaryInfo.Length -gt 0) {
        Write-Ok "VST3 binary: valid ($($binaryInfo.Length / 1MB).ToString('F1') MB)"
    } else {
        Write-ErrMsg "VST3 binary: empty file"
        $Valid = $false
    }
} else {
    Write-ErrMsg "VST3 binary: not found"
    $Valid = $false
}

# ── Summary ───────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "================================================================" -ForegroundColor White
Write-Host "  Build Summary - Windows x86_64" -ForegroundColor White
Write-Host "================================================================" -ForegroundColor White
Write-Host ""
Write-Host "  Plugin:       $PluginName"
Write-Host "  Version:      $Version"
Write-Host "  Vendor:       $Vendor"
Write-Host "  Target:       $Target"
Write-Host ""
Write-Host "  Output directory: $BuildDir" -ForegroundColor White
Write-Host ""
Write-Host "  VST3  ->  $Vst3Bundle" -ForegroundColor Green
Write-Host ""
Write-Host "  Bundle structure:" -ForegroundColor White
Write-Host "    $PluginName.vst3\"
Write-Host "      Contents\"
Write-Host "        x86_64-win\"
Write-Host "          $PluginName.vst3  (the DLL)"
Write-Host "        moduleinfo.json"
Write-Host ""

if ($Valid) {
    Write-Host "  All artifacts validated successfully!" -ForegroundColor Green
} else {
    Write-Host "  Some artifacts have issues - see errors above." -ForegroundColor Red
}

Write-Host ""
Write-Host "  Install location:" -ForegroundColor White
Write-Host "    VST3:  %COMMONPROGRAMFILES%\VST3\"
Write-Host ""

# ── Optional install ──────────────────────────────────────────────────────
if ($args.Count -gt 0 -and $args[0] -eq "--install") {
    Write-Step "Installing plugins..."

    $Vst3InstallDir = Join-Path $env:COMMONPROGRAMFILES "VST3"
    if (-not (Test-Path $Vst3InstallDir)) {
        New-Item -ItemType Directory -Path $Vst3InstallDir -Force | Out-Null
    }
    Copy-Item -Recurse -Force $Vst3Bundle $Vst3InstallDir
    Write-Ok "VST3 installed to $Vst3InstallDir"
}

if ($Valid) {
    exit 0
} else {
    exit 1
}
