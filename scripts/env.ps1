# Minimal Windows helper mirroring scripts/env.sh intent.
# Lumia needs LLVM 21 with llvm-sys-211. Set the prefix to your install.
#
# Examples:
#   $env:LLVM_SYS_211_PREFIX = "C:\Program Files\LLVM"
#   $env:LIBRARY_PATH = "$env:LLVM_SYS_211_PREFIX\lib"
#
# Full build notes: docs/BUILD.md (Linux+Nix is the primary local path;
# Windows CI installs the LLVM SDK separately).

if (-not $env:LLVM_SYS_211_PREFIX) {
    Write-Host "Set LLVM_SYS_211_PREFIX to your LLVM 21 root, then re-run."
    Write-Host "See docs/BUILD.md for toolchain details."
    return
}

Write-Host "LLVM_SYS_211_PREFIX=$env:LLVM_SYS_211_PREFIX"
if (-not $env:LIBRARY_PATH) {
    $env:LIBRARY_PATH = Join-Path $env:LLVM_SYS_211_PREFIX "lib"
}
Write-Host "LIBRARY_PATH=$env:LIBRARY_PATH"
Write-Host "Ready for cargo build (ensure clang/lld match LLVM 21)."
