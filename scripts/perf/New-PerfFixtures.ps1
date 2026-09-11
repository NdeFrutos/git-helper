#Requires -Version 5.1
<#
.SYNOPSIS
    Genera repositorios Git locales con 0, 300 o 2.000 cambios para medir rendimiento.

.DESCRIPTION
    Crea fixtures deterministas bajo un directorio de salida. Cada fixture incluye un
    commit inicial y archivos modificados sin stage. No envía datos fuera de la máquina.

.PARAMETER OutputRoot
    Directorio donde se crearán los fixtures (por defecto perf-fixtures junto al repo).

.PARAMETER ChangeCounts
    Cantidades de archivos modificados a generar.

.EXAMPLE
    .\New-PerfFixtures.ps1 -OutputRoot .\perf-fixtures -ChangeCounts 0,300,2000
#>
param(
    [string] $OutputRoot = (Join-Path (Split-Path $PSScriptRoot -Parent -Parent) 'perf-fixtures'),
    [int[]] $ChangeCounts = @(0, 300, 2000)
)

$ErrorActionPreference = 'Stop'

function Initialize-PerfRepository {
    param([string] $Path)

    if (Test-Path -LiteralPath $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }
    New-Item -ItemType Directory -Path $Path | Out-Null
    Push-Location $Path
    try {
        git init -b main | Out-Null
        git config core.autocrlf false
        git config user.name 'Git Helper Perf'
        git config user.email 'perf@git-helper.local'
        Set-Content -LiteralPath 'README.md' -Value '# perf fixture' -NoNewline
        git add README.md | Out-Null
        git commit -m 'initial' | Out-Null
    }
    finally {
        Pop-Location
    }
}

function Add-ModifiedFiles {
    param(
        [string] $RepositoryPath,
        [int] $Count
    )

    if ($Count -le 0) {
        return
    }

    $changesRoot = Join-Path $RepositoryPath 'changes'
    New-Item -ItemType Directory -Path $changesRoot -Force | Out-Null

    for ($index = 0; $index -lt $Count; $index += 1) {
        $shard = [math]::Floor($index / 100)
        $shardDirectory = Join-Path $changesRoot ("shard-{0:D3}" -f $shard)
        if (-not (Test-Path -LiteralPath $shardDirectory)) {
            New-Item -ItemType Directory -Path $shardDirectory | Out-Null
        }
        $filePath = Join-Path $shardDirectory ("file-{0:D5}.txt" -f $index)
        Set-Content -LiteralPath $filePath -Value ("change-{0}" -f $index) -NoNewline
    }
}

function Write-Manifest {
    param(
        [string] $RepositoryPath,
        [int] $ChangeCount,
        [string] $StageFile
    )

    $manifest = [ordered]@{
        change_count = $ChangeCount
        stage_file   = $StageFile
        created_at   = (Get-Date).ToUniversalTime().ToString('o')
        repository   = (Resolve-Path -LiteralPath $RepositoryPath).Path
    }
    $manifestPath = Join-Path $RepositoryPath 'perf-manifest.json'
    $manifest | ConvertTo-Json | Set-Content -LiteralPath $manifestPath -Encoding UTF8
}

New-Item -ItemType Directory -Path $OutputRoot -Force | Out-Null
$summary = @()

foreach ($changeCount in $ChangeCounts) {
    $fixtureName = if ($changeCount -eq 0) { 'clean' } else { "changes-$changeCount" }
    $repositoryPath = Join-Path $OutputRoot $fixtureName
    Write-Host "Creando fixture $fixtureName ($changeCount cambios)…"
    Initialize-PerfRepository -Path $repositoryPath
    Add-ModifiedFiles -RepositoryPath $repositoryPath -Count $changeCount

    $stageFile = $null
    if ($changeCount -gt 0) {
        $stageFile = Join-Path 'changes' 'shard-000' 'file-00000.txt'
    }
    Write-Manifest -RepositoryPath $repositoryPath -ChangeCount $changeCount -StageFile $stageFile
    $summary += [pscustomobject]@{
        Name         = $fixtureName
        ChangeCount  = $changeCount
        Repository   = (Resolve-Path -LiteralPath $repositoryPath).Path
        StageFile    = $stageFile
    }
}

$summaryPath = Join-Path $OutputRoot 'fixtures-summary.json'
$summary | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $summaryPath -Encoding UTF8
Write-Host "Fixtures listos en $OutputRoot"
Write-Output $summaryPath
