#Requires -Version 5.1
<#
.SYNOPSIS
    Ejecuta el benchmark Rust de la capa Git y compara stage frente al Git CLI.

.PARAMETER RepositoryPath
    Ruta al repositorio fixture.

.PARAMETER Iterations
    Repeticiones por métrica (por defecto 40).

.PARAMETER StageFile
    Ruta relativa dentro del repo para medir stage (opcional).

.PARAMETER OutputFile
    Ruta del JSON de salida. Por defecto se genera junto al repositorio.
#>
param(
    [Parameter(Mandatory)]
    [string] $RepositoryPath,
    [int] $Iterations = 40,
    [string] $StageFile,
    [string] $OutputFile
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$benchExe = Join-Path $repoRoot 'target\release\examples\bench.exe'
if (-not (Test-Path -LiteralPath $benchExe)) {
    Write-Host 'Compilando bench en release…'
    Push-Location $repoRoot
    try {
        cargo build --release --locked --example bench
    }
    finally {
        Pop-Location
    }
}

if (-not $StageFile) {
    $manifestPath = Join-Path $RepositoryPath 'perf-manifest.json'
    if (Test-Path -LiteralPath $manifestPath) {
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        if ($manifest.stage_file) {
            $StageFile = [string] $manifest.stage_file
        }
    }
}

$arguments = @(
    (Resolve-Path -LiteralPath $RepositoryPath).Path,
    '--iterations', $Iterations,
    '--json'
)
if ($StageFile) {
    $arguments += @('--stage-file', $StageFile)
}

$output = & $benchExe @arguments
if ($LASTEXITCODE -ne 0) {
    throw "bench.exe terminó con código $LASTEXITCODE"
}

if (-not $OutputFile) {
    $fixtureName = Split-Path $RepositoryPath -Leaf
    $OutputFile = Join-Path $RepositoryPath "bench-$fixtureName.json"
}
$output | Set-Content -LiteralPath $OutputFile -Encoding UTF8
Write-Output $OutputFile
