<#
.SYNOPSIS
    Garantiza que ~/.cargo/bin contiene exactamente la versión requerida de cargo-wix.

.DESCRIPTION
    La caché de Cargo incluye ~/.cargo/bin y ~/.cargo/.crates.toml, así que una
    restauración por prefijo puede traer un cargo-wix de otra versión. Este script
    comprueba la versión realmente instalada y reinstala si no coincide, de modo que
    cambiar CARGO_WIX_VERSION nunca reutilice en silencio otro binario.

    La versión se lee de `cargo install --list`, que es el registro que mantiene el
    propio Cargo en ~/.cargo/.crates.toml, y no de un flag del binario.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Version
)

$ErrorActionPreference = "Stop"

$executable = Join-Path $env:USERPROFILE ".cargo\bin\cargo-wix.exe"

function Get-InstalledCargoWixVersion {
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) { return $null }

    $listed = (cargo install --list 2>&1 | Out-String)
    if ($listed -match '(?m)^cargo-wix v(?<version>\d+\.\d+\.\d+)') { return $Matches.version }
    return $null
}

$installed = Get-InstalledCargoWixVersion

if ($installed -eq $Version) {
    Write-Host "Reutilizando cargo-wix $installed restaurado desde la caché."
    return
}

$found = if ($installed) { "cargo-wix $installed" } else { "ningún cargo-wix registrado" }
Write-Host "Se encontró $found; se instala la versión requerida $Version."
cargo install cargo-wix --version $Version --locked --force
if ($LASTEXITCODE -ne 0) { throw "No se pudo instalar cargo-wix $Version." }

$installed = Get-InstalledCargoWixVersion
if ($installed -ne $Version) {
    throw "Tras instalar, cargo registra cargo-wix '$installed' en lugar de ${Version}."
}
Write-Host "cargo-wix $Version instalado y verificado."
