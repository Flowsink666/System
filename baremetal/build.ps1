param([switch]$SkipTargetInstall)
$ErrorActionPreference = 'Stop'
$nativeRoot = $PSScriptRoot
$projectRoot = Split-Path -Parent $nativeRoot
$outputRoot = Join-Path $projectRoot 'target/baremetal'
if (-not $SkipTargetInstall) {
    & rustup target add x86_64-unknown-uefi
    if ($LASTEXITCODE -ne 0) { throw 'UEFI target installation failed' }
}
& cargo build --manifest-path (Join-Path $nativeRoot 'Cargo.toml') --target x86_64-unknown-uefi --target-dir $outputRoot --release --offline
if ($LASTEXITCODE -ne 0) { throw 'Native kernel build failed' }
$efiPayload = Join-Path $outputRoot 'x86_64-unknown-uefi/release/mini_os_native.efi'
& node (Join-Path $nativeRoot 'scripts/make-image.js') $efiPayload (Join-Path $outputRoot 'mini-os.img')
if ($LASTEXITCODE -ne 0) { throw 'Image creation failed' }
