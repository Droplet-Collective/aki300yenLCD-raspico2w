param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-fA-F]{16}$')]
    [string]$ExpectedSerial,

    [string]$Picotool = 'picotool'
)

$ErrorActionPreference = 'Stop'
$repo = $PSScriptRoot
$elf = Join-Path $repo 'target\thumbv8m.main-none-eabihf\release\sd_bmp_viewer'
$serial = $ExpectedSerial.ToUpperInvariant()

Push-Location $repo
try {
    & cargo build --release --bin sd_bmp_viewer
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    if (-not (Test-Path -LiteralPath $elf -PathType Leaf)) { throw "ELF not found: $elf" }

    # -f works after firmware with the USB reset interface is installed.
    # In BOOTSEL mode, the same command can perform the initial installation.
    & $Picotool load --ser $serial -f -u -v -x -t elf $elf
    if ($LASTEXITCODE -ne 0) { throw "picotool load failed ($LASTEXITCODE)" }
}
finally {
    Pop-Location
}
