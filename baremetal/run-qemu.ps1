param([string]$QemuPath, [string]$FirmwarePath, [string]$VarsPath, [switch]$NoBuild, [switch]$Headless, [int]$QmpPort = 0)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$outputRoot = Join-Path $projectRoot 'target/baremetal'
if (-not $NoBuild) { & (Join-Path $PSScriptRoot 'build.ps1') }
if (-not $QemuPath) {
    $command = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue
    if ($command) { $QemuPath = $command.Source }
    elseif (Test-Path (Join-Path $projectRoot 'target/tools/qemu/qemu-system-x86_64.exe')) { $QemuPath = Join-Path $projectRoot 'target/tools/qemu/qemu-system-x86_64.exe' }
    elseif (Test-Path 'C:/Program Files/qemu/qemu-system-x86_64.exe') { $QemuPath = 'C:/Program Files/qemu/qemu-system-x86_64.exe' }
    else { throw 'QEMU not found. Install QEMU or pass -QemuPath.' }
}
if (-not $FirmwarePath) {
    foreach ($relative in @('share/edk2-x86_64-code.fd', 'edk2-x86_64-code.fd', 'share/qemu/edk2-x86_64-code.fd')) {
        $candidate = Join-Path (Split-Path -Parent $QemuPath) $relative
        if (Test-Path $candidate) { $FirmwarePath = $candidate; break }
    }
    if (-not $FirmwarePath) { throw 'UEFI firmware not found. Pass -FirmwarePath to an OVMF/EDK2 x86_64 code image.' }
}
$imagePath = Join-Path $outputRoot 'mini-os.img'
if (-not (Test-Path $imagePath)) { throw 'Disk image missing. Run baremetal/build.ps1 first.' }
if (-not $VarsPath) {
    foreach ($name in @('edk2-i386-vars.fd', 'OVMF_VARS.fd', 'OVMF_VARS_4M.fd')) {
        $candidate = Join-Path (Split-Path -Parent $FirmwarePath) $name
        if (Test-Path $candidate) { $VarsPath = $candidate; break }
    }
    if (-not $VarsPath) { throw 'UEFI variable-store template not found. Pass -VarsPath matching your code firmware.' }
}
$localVars = Join-Path $outputRoot 'uefi-vars.fd'
if (-not (Test-Path $localVars)) { Copy-Item -LiteralPath $VarsPath -Destination $localVars }
$qemuArguments = @('-machine', 'q35', '-m', '256M', '-smp', '1', '-accel', 'tcg',
    '-drive', "if=pflash,format=raw,unit=0,readonly=on,file=$FirmwarePath",
    '-drive', "if=pflash,format=raw,unit=1,file=$localVars",
    '-drive', "format=raw,file=$imagePath", '-vga', 'std', '-serial', "file:$(Join-Path $outputRoot 'serial.log')", '-no-reboot', '-net', 'none')
if ($Headless) { $qemuArguments += @('-display','none') }
if ($QmpPort -gt 0) { $qemuArguments += @('-qmp',"tcp:127.0.0.1:$QmpPort,server=on,wait=off") }
Write-Host 'Mini-OS: F1-F5 switch apps; mouse clicks; A allocate; F free; N new task; F4 opens terminal.'
& $QemuPath @qemuArguments
if ($LASTEXITCODE -ne 0) { throw "QEMU exited with code $LASTEXITCODE" }
