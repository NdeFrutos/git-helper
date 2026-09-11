#Requires -Version 5.1
<#
.SYNOPSIS
    Comprueba que Git Helper en reposo no lanza procesos git periódicos.

.DESCRIPTION
    Abre Git Helper con un repositorio, espera el periodo de muestreo y registra cuántos
    procesos git.exe existen en cada intervalo. Útil para validar que no hay auto-fetch ni
    refrescos espurios en reposo.

.PARAMETER Executable
    Ruta a git-helper.exe compilado en release.

.PARAMETER RepositoryPath
    Repositorio a abrir (se pasa por variable de entorno GH_PERF_OPEN_REPO).

.PARAMETER SampleSeconds
    Duración total del muestreo.

.PARAMETER IntervalSeconds
    Intervalo entre muestras.
#>
param(
    [string] $Executable = (Join-Path (Split-Path $PSScriptRoot -Parent -Parent) 'target\release\git-helper.exe'),
    [Parameter(Mandatory)]
    [string] $RepositoryPath,
    [int] $SampleSeconds = 60,
    [int] $IntervalSeconds = 5
)

$ErrorActionPreference = 'Stop'

$resolvedExecutable = (Resolve-Path -LiteralPath $Executable).Path
$resolvedRepository = (Resolve-Path -LiteralPath $RepositoryPath).Path
$env:GH_PERF_OPEN_REPO = $resolvedRepository

$application = Start-Process -FilePath $resolvedExecutable -PassThru
Start-Sleep -Seconds 8

$samples = @()
$deadline = (Get-Date).AddSeconds($SampleSeconds)

try {
    while ((Get-Date) -lt $deadline -and -not $application.HasExited) {
        $gitProcesses = @(Get-Process -Name git -ErrorAction SilentlyContinue)
        $samples += [pscustomobject]@{
            timestamp_utc = (Get-Date).ToUniversalTime().ToString('o')
            git_process_count = $gitProcesses.Count
            git_pids = ($gitProcesses | ForEach-Object { $_.Id }) -join ','
        }
        Start-Sleep -Seconds $IntervalSeconds
    }
}
finally {
    if (-not $application.HasExited) {
        $application.CloseMainWindow() | Out-Null
        if (-not $application.WaitForExit(3000)) {
            Stop-Process -Id $application.Id -Force
        }
    }
    Remove-Item Env:GH_PERF_OPEN_REPO -ErrorAction SilentlyContinue
}

$maxGit = ($samples | Measure-Object -Property git_process_count -Maximum).Maximum
$result = [ordered]@{
    repository = $resolvedRepository
    sample_seconds = $SampleSeconds
    interval_seconds = $IntervalSeconds
    max_git_process_count = $maxGit
    samples = $samples
    passed = ($maxGit -eq 0)
}

$outputPath = Join-Path $resolvedRepository 'idle-git-processes.json'
$result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $outputPath -Encoding UTF8

if (-not $result.passed) {
    Write-Warning "Se detectaron hasta $maxGit procesos git.exe en reposo. Revisar idle-git-processes.json"
}
Write-Output $outputPath
