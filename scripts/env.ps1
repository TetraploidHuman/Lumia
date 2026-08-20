# Minimal Windows helper mirroring scripts/env.sh intent.
# Lumia needs LLVM 21 with llvm-sys-211. Set the prefix to your install.
#
# Examples:
#   $env:LLVM_SYS_211_PREFIX = "C:\Program Files\LLVM"
#   . .\scripts\env.ps1
#
# Full build notes: docs/BUILD.md (Linux/Nix defaults to shared libLLVM via
# `lumia`'s `llvm-dynamic`). On Windows use:
#   cargo build -p lumia --no-default-features --features codegen
# Windows CI installs the LLVM SDK separately.

if (-not $env:LLVM_SYS_211_PREFIX) {
    Write-Host "Set LLVM_SYS_211_PREFIX to your LLVM 21 root, then re-run."
    Write-Host "See docs/BUILD.md for toolchain details."
    return
}

$prefix = $env:LLVM_SYS_211_PREFIX
$binDir = Join-Path $prefix "bin"
$libDir = Join-Path $prefix "lib"

# Match env.sh: put LLVM tools first on PATH (clang / lld / llvm-config).
if (Test-Path $binDir) {
    $env:PATH = "$binDir;$env:PATH"
} else {
    Write-Warning "LLVM bin dir missing: $binDir"
}

if (-not $env:LIBRARY_PATH) {
    $env:LIBRARY_PATH = $libDir
} elseif ($env:LIBRARY_PATH -notlike "*$libDir*") {
    $env:LIBRARY_PATH = "$libDir;$env:LIBRARY_PATH"
}
# MSVC linkers often look at LIB; prepend when missing so rustc can find LLVM libs.
if (-not $env:LIB) {
    $env:LIB = $libDir
} elseif ($env:LIB -notlike "*$libDir*") {
    $env:LIB = "$libDir;$env:LIB"
}

Write-Host "LLVM_SYS_211_PREFIX=$prefix"
Write-Host "PATH prepended: $binDir"
Write-Host "LIBRARY_PATH=$env:LIBRARY_PATH"
Write-Host "LIB=$env:LIB"

$clang = Get-Command clang -ErrorAction SilentlyContinue
$llvmConfig = Get-Command llvm-config -ErrorAction SilentlyContinue
if (-not $clang) {
    Write-Warning "clang not found on PATH after env setup"
}
if (-not $llvmConfig) {
    Write-Warning "llvm-config not found on PATH after env setup"
}
Write-Host "Ready for cargo build (ensure clang/lld match LLVM 21)."
