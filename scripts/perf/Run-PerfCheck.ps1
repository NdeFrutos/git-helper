#Requires -Version 5.1
<#
.SYNOPSIS
    Orquesta la comprobación ligera de rendimiento de Git Helper en Windows release.

.DESCRIPTION
    Genera fixtures, ejecuta el benchmark headless, registra metadatos de hardware y SHA,
    y deja un informe JSON consolidado. Las mediciones de UI (primer frame, pestañas, scroll)
    siguen siendo manuales; ver docs/performance-check.md.

.PARAMETER OutputRoot
    Directorio de salida para fixtures y resultados.

.PARAMETER Iterations
    Repeticiones por métrica del benchmark Rust.

.PARAMETER SkipIdleCheck
    Omite la comprobación de procesos git en reposo (requiere ventana visible).
#>
param(
    [string] $OutputRoot = (Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'perf-results'),
    [int[]] $ChangeCounts = @(0, 300, 2000),
    [int] $Iterations = 40,
    [switch] $SkipIdleCheck
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$fixturesRoot = Join-Path $OutputRoot 'fixtures'
$benchRoot = Join-Path $OutputRoot 'bench'
New-Item -ItemType Directory -Path $fixturesRoot, $benchRoot -Force | Out-Null

function Get-GitSha {
    Push-Location $repoRoot
    try {
        return (git rev-parse HEAD).Trim()
    }
    finally {
        Pop-Location
    }
}

function Get-HardwareSummary {
    $computer = Get-CimInstance Win32_ComputerSystem
    $processor = Get-CimInstance Win32_Processor | Select-Object -First 1
    return [ordered]@{
        machine_name = $env:COMPUTERNAME
        os           = (Get-CimInstance Win32_OperatingSystem).Caption
        cpu          = $processor.Name
        logical_cpus = $computer.NumberOfLogicalProcessors
        ram_gb       = [math]::Round($computer.TotalPhysicalMemory / 1GB, 1)
    }
}

Write-Host 'Compilando Git Helper en release (con feature perf-hooks)…'
Push-Location $repoRoot
try {
    # perf-hooks habilita GH_PERF_OPEN_REPO, necesario para la medición en reposo.
    # El binario publicado se construye sin esta feature.
    cargo build --release --locked --features perf-hooks
    cargo build --release --locked --example bench
}
finally {
    Pop-Location
}

$fixturesSummary = & (Join-Path $PSScriptRoot 'New-PerfFixtures.ps1') `
    -OutputRoot $fixturesRoot `
    -ChangeCounts $ChangeCounts

$benchReports = @()
foreach ($changeCount in $ChangeCounts) {
    $fixtureName = if ($changeCount -eq 0) { 'clean' } else { "changes-$changeCount" }
    $repositoryPath = Join-Path $fixturesRoot $fixtureName
    $outputFile = Join-Path $benchRoot "bench-$fixtureName.json"
    Write-Host "Midiendo $fixtureName…"
    $benchReports += Get-Content -LiteralPath (
        & (Join-Path $PSScriptRoot 'Measure-GitBench.ps1') `
            -RepositoryPath $repositoryPath `
            -Iterations $Iterations `
            -OutputFile $outputFile
    ) -Raw | ConvertFrom-Json
}

$idleReport = $null
if (-not $SkipIdleCheck) {
    $idleFixture = Join-Path $fixturesRoot 'clean'
    Write-Host 'Comprobando procesos git en reposo (60 s)…'
    $idlePath = & (Join-Path $PSScriptRoot 'Measure-IdleGitProcesses.ps1') `
        -RepositoryPath $idleFixture `
        -SampleSeconds 60 `
        -IntervalSeconds 5
    $idleReport = Get-Content -LiteralPath $idlePath -Raw | ConvertFrom-Json
}

$report = [ordered]@{
    recorded_at_utc = (Get-Date).ToUniversalTime().ToString('o')
    git_sha         = Get-GitSha
    git_helper_version = (Select-String -Path (Join-Path $repoRoot 'Cargo.toml') -Pattern '^version = "(.+)"' | ForEach-Object { $_.Matches[0].Groups[1].Value })
    build_profile   = 'release+perf-hooks'
    hardware        = Get-HardwareSummary
    sampling        = [ordered]@{
        bench_iterations = $Iterations
        idle_sample_seconds = if ($idleReport) { 60 } else { $null }
    }
    targets         = [ordered]@{
        first_frame_ms = 1000
        tab_switch_p95_ms = 100
        notes = 'Objetivos calibrables; desviaciones deben justificarse en el informe.'
    }
    bench_reports   = $benchReports
    idle_git_processes = $idleReport
    manual_checks   = @(
        'Primer frame visible (<1 s): medir con trazas RUST_LOG=git_helper=debug,info o cronómetro.',
        'Cambio de pestaña Cambios/Historial p95 (<100 ms): medir con trazas o grabación de pantalla.',
        'Scroll en listas grandes: validar suavidad visual con fixture changes-2000.'
    )
}

$reportPath = Join-Path $OutputRoot 'perf-report.json'
$report | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $reportPath -Encoding UTF8
Write-Host "Informe consolidado: $reportPath"
Write-Output $reportPath
