#Requires -Version 5.1
<#
.SYNOPSIS
    Resume latencias de operaciones Git a partir del log de tracing de Git Helper.

.DESCRIPTION
    Lee %LOCALAPPDATA%\GitHelper\logs\git-helper.log (o una ruta explícita) y extrae
    elapsed_ms de entradas "Proceso finalizado". No incluye contenido del repositorio.

.PARAMETER LogPath
    Ruta al archivo de log. Por defecto usa el log diario de Git Helper.

.PARAMETER Label
    Filtra por operation= (por ejemplo git-status, git-stage).
#>
param(
    [string] $LogPath,
    [string] $Label
)

$ErrorActionPreference = 'Stop'

if (-not $LogPath) {
    $logDirectory = Join-Path $env:LOCALAPPDATA 'GitHelper\logs'
    if (-not (Test-Path -LiteralPath $logDirectory)) {
        throw "No se encontró el directorio de logs en $logDirectory"
    }
    $LogPath = Get-ChildItem -LiteralPath $logDirectory -Filter 'git-helper.log*' |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}

if (-not (Test-Path -LiteralPath $LogPath)) {
    throw "Log no encontrado: $LogPath"
}

$pattern = 'operation=(?<operation>[^\s]+).*elapsed_ms=(?<elapsed>\d+)'
$entries = Select-String -LiteralPath $LogPath -Pattern $pattern -AllMatches |
    ForEach-Object {
        foreach ($match in $_.Matches) {
            [pscustomobject]@{
                operation = $match.Groups['operation'].Value
                elapsed_ms = [int] $match.Groups['elapsed'].Value
            }
        }
    }

if ($Label) {
    $entries = @($entries | Where-Object { $_.operation -eq $Label })
}

if ($entries.Count -eq 0) {
    Write-Warning 'No se encontraron entradas de proceso finalizado.'
    return
}

$grouped = $entries | Group-Object operation | ForEach-Object {
    $values = @($_.Group | ForEach-Object { $_.elapsed_ms } | Sort-Object)
    $count = $values.Count
    $p50 = $values[[math]::Floor(($count - 1) * 0.50)]
    $p95 = $values[[math]::Floor(($count - 1) * 0.95)]
    [pscustomobject]@{
        operation = $_.Name
        samples = $count
        min_ms = ($values | Measure-Object -Minimum).Minimum
        p50_ms = $p50
        p95_ms = $p95
        max_ms = ($values | Measure-Object -Maximum).Maximum
        mean_ms = [math]::Round(($values | Measure-Object -Average).Average, 1)
    }
}

$grouped | Format-Table -AutoSize
$grouped | ConvertTo-Json | Write-Output
