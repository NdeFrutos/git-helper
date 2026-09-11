<#
.SYNOPSIS
    Publica métricas de la caché de Cargo de un job: duración, acierto de caché y
    tamaño en disco de las rutas cacheadas.

.DESCRIPTION
    Lee CACHE_HIT y MATCHED_KEY del entorno (salidas de actions/cache) y escribe un
    bloque en el resumen del job. Además expone duration_seconds, cache_hit y
    cache_size_mb como salidas del paso para que otros jobs las usen. La clave
    primaria no se repite aquí: la imprime el propio paso de actions/cache.

    Los tamaños son los del contenido descomprimido en disco, no los del archivo
    que GitHub almacena; sirven para vigilar el límite de 10 GB por repositorio.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Phase,
    [Parameter(Mandatory = $true)][string]$StartedAt
)

$ErrorActionPreference = "Stop"

function Get-PathSizeMb {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    if (Test-Path -LiteralPath $Path -PathType Leaf) {
        return [math]::Round((Get-Item -LiteralPath $Path).Length / 1MB, 1)
    }

    $total = 0
    foreach ($file in [System.IO.Directory]::EnumerateFiles($Path, "*", [System.IO.SearchOption]::AllDirectories)) {
        try { $total += [System.IO.FileInfo]::new($file).Length } catch { }
    }
    return [math]::Round($total / 1MB, 1)
}

$cachedPaths = [ordered]@{
    "~/.cargo/registry" = Join-Path $env:USERPROFILE ".cargo\registry"
    "~/.cargo/git"      = Join-Path $env:USERPROFILE ".cargo\git"
    "~/.cargo/bin"      = Join-Path $env:USERPROFILE ".cargo\bin"
    "target"            = "target"
}

$rows = foreach ($entry in $cachedPaths.GetEnumerator()) {
    [pscustomobject]@{ Path = $entry.Key; SizeMb = Get-PathSizeMb -Path $entry.Value }
}
$totalMb = [math]::Round(($rows | Measure-Object -Property SizeMb -Sum).Sum, 1)

$duration = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds() - [long]$StartedAt
$exactHit = if ($env:CACHE_HIT -eq "true") { "sí" } else { "no" }
$matched = if ([string]::IsNullOrWhiteSpace($env:MATCHED_KEY)) { "ninguna (ejecución fría)" } else { $env:MATCHED_KEY }

$summary = @(
    "## Caché de Cargo: $Phase",
    "",
    "- Duración del job: $duration s",
    "- Acierto exacto: $exactHit",
    "- Clave restaurada: ``$matched``",
    "- Tamaño total en disco de las rutas cacheadas: $totalMb MB",
    "",
    "Ruta | Tamaño (MB)",
    "--- | ---:"
)
$summary += $rows | ForEach-Object { "``$($_.Path)`` | $($_.SizeMb)" }

if ($env:GITHUB_STEP_SUMMARY) {
    $summary -join "`n" | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Append -Encoding utf8
}
$summary -join "`n" | Write-Host

if ($env:GITHUB_OUTPUT) {
    @(
        "duration_seconds=$duration",
        "cache_hit=$($env:CACHE_HIT)",
        "cache_size_mb=$totalMb"
    ) | Out-File -FilePath $env:GITHUB_OUTPUT -Append -Encoding utf8
}
