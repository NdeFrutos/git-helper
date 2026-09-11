<#
.SYNOPSIS
    Garantiza que ~/.cargo/bin contiene exactamente la versión requerida de cargo-wix.

.DESCRIPTION
    La caché de Cargo incluye ~/.cargo/bin, así que una restauración parcial puede
    traer un cargo-wix de otra versión. Este script comprueba la versión real del
    binario y reinstala si no coincide, de modo que cambiar CARGO_WIX_VERSION nunca
    reutilice silenciosamente otro binario.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Version
)

$ErrorActionPreference = "Stop"

$executable = Join-Path $env:USERPROFILE ".cargo\bin\cargo-wix.exe"
$installed = $null

if (Test-Path -LiteralPath $executable -PathType Leaf) {
    $reported = & $executable wix --version 2>&1
    if ($LASTEXITCODE -eq 0 -and "$reported" -match '(?<version>\d+\.\d+\.\d+)') {
        $installed = $Matches.version
    }
}

if ($installed -eq $Version) {
    Write-Host "Reutilizando cargo-wix $installed restaurado desde la caché."
    return
}

$found = if ($installed) { "cargo-wix $installed" } else { "ningún cargo-wix utilizable" }
Write-Host "Se encontró $found; se instala la versión requerida $Version."
cargo install cargo-wix --version $Version --locked --force
if ($LASTEXITCODE -ne 0) { throw "No se pudo instalar cargo-wix $Version." }

$reported = & $executable wix --version 2>&1
if ($LASTEXITCODE -ne 0 -or "$reported" -notmatch [regex]::Escape($Version)) {
    throw "cargo-wix instalado no reporta la versión $Version: $reported"
}
Write-Host "cargo-wix $Version instalado y verificado."
