# Comprobación ligera de rendimiento — Git Helper

Procedimiento reproducible para medir el rendimiento del panel sin telemetría externa.
Consolida mediciones headless (capa Git) y comprobaciones manuales de UI en Windows release.

**Baseline de auditoría original:** `714a7ba` (informe en [performance-audit.md](performance-audit.md)).
**Revisión medida por este procedimiento:** anotar el `git_sha` del informe generado.

---

## Requisitos

| Elemento | Detalle |
|---|---|
| SO | Windows 10/11 x64 |
| Build | `cargo build --release --locked` (la comprobación en reposo necesita además `--features perf-hooks`, ver paso 3) |
| Git | Git for Windows en `PATH` |
| Hardware | Anotar CPU, RAM y escala DPI en el informe |
| Duración típica | ~15 min (fixtures + bench) + 5 min comprobaciones manuales |

---

## Objetivos calibrables

| Métrica | Objetivo inicial | Tipo |
|---|---|---|
| Primer frame visible | < 1 s | Manual / trazas |
| Cambio pestaña Cambios↔Historial p95 | < 100 ms | Manual / trazas |
| Snapshot sin historial p95 | Baseline documentada | Automática (`bench`) |
| Stage + refresco p95 vs Git CLI | Sobrecoste documentado | Automática (`bench`) |
| Procesos `git.exe` en reposo (60 s) | 0 | Automática (`Measure-IdleGitProcesses`) |

Las desviaciones deben justificarse en el informe (hardware lento, antivirus, repo en red, etc.).

---

## Ejecución automática (recomendada)

Desde la raíz del repositorio, en PowerShell:

```powershell
.\scripts\perf\Run-PerfCheck.ps1 -OutputRoot .\perf-results -Iterations 40
```

Esto:

1. Compila `git-helper.exe` y `bench.exe` en release.
2. Genera fixtures con **0**, **300** y **2.000** archivos versionados y modificados sin stage.
3. Ejecuta el benchmark Rust por fixture y guarda JSON en `perf-results/bench/`.
4. Comprueba procesos `git.exe` en reposo durante 60 s (fixture `clean`).
5. Escribe `perf-results/perf-report.json` con SHA, hardware y resultados.

Para omitir la comprobación de reposo (headless CI, sin ventana):

```powershell
.\scripts\perf\Run-PerfCheck.ps1 -SkipIdleCheck
```

---

## Pasos individuales

### 1. Generar fixtures

```powershell
.\scripts\perf\New-PerfFixtures.ps1 -OutputRoot .\perf-fixtures -ChangeCounts 0,300,2000
```

Cada fixture incluye `perf-manifest.json` con la ruta relativa del archivo usado en stage.
Los archivos se versionan en un commit base y después se modifican, de modo que el fixture
tiene N cambios sobre contenido seguido por Git (`git status` recorre y compara cada archivo);
con ficheros sin seguir, Git colapsa el directorio en una sola entrada y la medición no
representaría el escenario.

### 2. Benchmark de la capa Git

```powershell
cargo build --release --locked --example bench
.\target\release\examples\bench.exe .\perf-fixtures\changes-300 `
    --iterations 40 --stage-file changes\shard-000\file-00000.txt --json
```

Métricas reportadas (p50/p95):

- Coste de lanzar `git --version`
- `status`, `snapshot`, `snapshot_with_history`
- Flujo app completo: `stage` → `snapshot` → `unstage` (`stage_flow_ms`)
- Flujo app sin refresco: `stage` → `unstage` (`stage_only_ms`)
- Baseline CLI: `git add` → `git restore --staged` (`git_cli_stage_ms`)
- **Sobrecoste app** (`stage_overhead_ms`) = `stage_only_ms` − `git_cli_stage_ms`, es decir el
  mismo trabajo en ambos lados. El snapshot de `stage_flow_ms` no entra en la comparación
  porque el Git CLI no hace ese refresco; se reporta aparte como coste del flujo de la app.

En el fixture `clean` (0 cambios) no hay nada que preparar: el bench avisa por `stderr`, omite las
métricas `stage_*` del JSON y mide solo `status`/`snapshot`. Para forzar una medición de stage en
un repositorio sin cambios, pasar `--stage-file <ruta-relativa>`.

### 3. Procesos Git en reposo

El gancho `GH_PERF_OPEN_REPO` **solo existe si el binario se compila con la feature
`perf-hooks`**; el binario publicado (MSI) se construye sin ella y por tanto ignora la
variable. Hay que construir primero un binario de medición:

```powershell
cargo build --release --locked --features perf-hooks
```

Con ese binario y un repositorio local:

```powershell
$env:GH_PERF_OPEN_REPO = (Resolve-Path .\perf-fixtures\clean).Path
.\scripts\perf\Measure-IdleGitProcesses.ps1 -RepositoryPath $env:GH_PERF_OPEN_REPO
Remove-Item Env:GH_PERF_OPEN_REPO
```

`GH_PERF_OPEN_REPO` abre el repositorio al iniciar (solo para mediciones; no es telemetría).
Sin auto-fetch ni actividad externa, **no debe aparecer ningún `git.exe`** durante el muestreo.

El script solo cuenta los `git.exe` iniciados después de lanzar la app, pero no puede
distinguir el proceso padre: cerrar IDEs, agentes y shells que puedan ejecutar Git antes de
medir, o el recuento saldrá contaminado.

### 4. Analizar trazas locales

Las trazas `Proceso finalizado` se emiten en nivel `debug`: con el nivel por defecto (`info`)
el log no contendrá muestras. Ejecutar la app con logging detallado (no registra contenido del
repo):

```powershell
$env:RUST_LOG = 'git_helper=debug,info'
.\target\release\git-helper.exe
```

Tras interactuar (stage, cambio de pestaña, scroll), resumir latencias:

```powershell
.\scripts\perf\Analyze-TraceLog.ps1
.\scripts\perf\Analyze-TraceLog.ps1 -Label git-status
```

Los logs rotativos están en `%LOCALAPPDATA%\GitHelper\logs\`.

---

## Comprobaciones manuales de UI

Registrar en el informe como **verificación manual** (no sustituyen al bench):

| Escenario | Procedimiento |
|---|---|
| Primer frame | Cronómetro desde doble clic en exe hasta ventana usable; repetir 5 veces |
| 1 vs 10 repos | Abrir 1 y luego 10 pestañas; anotar tiempo hasta lista lista |
| Ráfaga externa | Editar/guardar en IDE 10 veces seguidas; contar refrescos percibidos |
| Stage → confirmación | Clic en Stage de un archivo; medir hasta fila actualizada |
| Scroll | Fixture `changes-2000`; scroll continuo 10 s; anotar tirones |
| Cambio de pestaña | Alternar Cambios/Historial 20 veces; estimar p95 |

Usar [visual-validation.md](visual-validation.md) como checklist complementario.

---

## Estructura del informe

Separar siempre tres categorías en `perf-report.json` o en notas del PR:

1. **Resultados medidos** — JSON del bench, idle check, trazas analizadas.
2. **Hipótesis** — interpretaciones no confirmadas (p. ej. impacto de antivirus).
3. **Verificaciones manuales** — UI, DPI, scroll; declarar limitaciones del entorno.

Campos obligatorios para que otro agente repita la medición:

- `git_sha`, `git_helper_version`, `build_profile`
- Resumen de hardware (`machine_name`, CPU, RAM)
- `iterations` y duración de muestreo idle
- Rutas a fixtures y JSON de bench por escenario (0 / 300 / 2.000 cambios)

---

## Entorno Linux / CI

En entornos sin GPUI interactivo:

```bash
cargo build --release --locked --example bench
cargo test --all-targets --locked perf_
```

El example `bench` y las pruebas `perf_*` validan la capa Git headless.
Las comprobaciones de UI y procesos idle **deben documentarse como no ejecutadas** en ese entorno,
no darse por pasadas.

---

## Relación con la auditoría

[performance-audit.md](performance-audit.md) lista hallazgos del baseline `714a7ba` y su estado
actual (resuelto / pendiente / parcial). Este procedimiento no bloquea correcciones anteriores;
sirve para repetir mediciones tras cada cambio relevante de rendimiento.
