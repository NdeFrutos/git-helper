//! Benchmark reproducible de la capa Git usada por Git Helper.
//!
//! No registra contenido del repositorio; solo tiempos y etiquetas de operación.
//! Ver [docs/performance-check.md](../docs/performance-check.md).

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use git_helper::{git::GitClient, process::CancellationToken};
use serde::Serialize;

const DEFAULT_ITERATIONS: usize = 40;
const WARMUP_ITERATIONS: usize = 3;

#[derive(Serialize)]
struct BenchReport {
    metadata: Metadata,
    measurements: Measurements,
}

#[derive(Serialize)]
struct Metadata {
    repository: String,
    iterations: usize,
    warmup_iterations: usize,
    build_profile: &'static str,
    git_helper_version: String,
    platform: String,
    recorded_at: String,
}

#[derive(Serialize)]
struct Measurements {
    #[serde(rename = "spawn_cost_ms")]
    spawn_cost: PercentileStats,
    #[serde(rename = "status_ms")]
    status: PercentileStats,
    #[serde(rename = "snapshot_ms")]
    snapshot: PercentileStats,
    #[serde(rename = "snapshot_with_history_ms")]
    snapshot_with_history: PercentileStats,
    /// Métricas de stage: ausentes si el repositorio no tiene cambios que preparar.
    #[serde(flatten)]
    stage: Option<StageMeasurements>,
}

#[derive(Serialize)]
struct StageMeasurements {
    #[serde(rename = "stage_flow_ms")]
    stage_flow: PercentileStats,
    #[serde(rename = "stage_only_ms")]
    stage_only: PercentileStats,
    #[serde(rename = "git_cli_stage_ms")]
    git_cli_stage: PercentileStats,
    #[serde(rename = "stage_overhead_ms")]
    stage_overhead: PercentileStats,
}

#[derive(Serialize)]
struct PercentileStats {
    samples: usize,
    #[serde(rename = "min_ms")]
    minimum: f64,
    #[serde(rename = "p50_ms")]
    median: f64,
    #[serde(rename = "p95_ms")]
    tail: f64,
    #[serde(rename = "max_ms")]
    maximum: f64,
    #[serde(rename = "mean_ms")]
    average: f64,
}

struct BenchOptions {
    repository: PathBuf,
    iterations: usize,
    json_output: bool,
    stage_file: Option<PathBuf>,
}

fn main() {
    let options = parse_options();
    let stage_relative = resolve_stage_file(&options);
    let report = run_benchmark(
        &options.repository,
        options.iterations,
        stage_relative.as_deref(),
    );
    emit_report(&report, options.json_output);
}

fn parse_options() -> BenchOptions {
    let mut args = std::env::args().skip(1);
    let repository = args
        .next()
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .canonicalize()
        .expect("el repositorio debe existir");
    let mut iterations = DEFAULT_ITERATIONS;
    let mut json_output = false;
    let mut stage_file = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--iterations" => {
                iterations = args
                    .next()
                    .expect("--iterations requiere un valor")
                    .parse()
                    .expect("iteraciones debe ser un entero");
            }
            "--json" => json_output = true,
            "--stage-file" => {
                stage_file = Some(
                    args.next()
                        .map(PathBuf::from)
                        .expect("--stage-file requiere una ruta relativa"),
                );
            }
            other => {
                eprintln!("argumento desconocido: {other}");
                std::process::exit(2);
            }
        }
    }

    if iterations == 0 {
        // Coherente con el resto de errores de argumentos: código 2, no panic.
        eprintln!("--iterations debe ser al menos 1; no hay percentiles sin muestras");
        std::process::exit(2);
    }

    BenchOptions {
        repository,
        iterations,
        json_output,
        stage_file,
    }
}

fn resolve_stage_file(options: &BenchOptions) -> Option<PathBuf> {
    if let Some(stage_file) = &options.stage_file {
        return Some(stage_file.clone());
    }
    let candidate = discover_stage_candidate(&options.repository);
    if candidate.is_none() {
        // El fixture `clean` (0 cambios) es un escenario válido del procedimiento: se miden
        // status y snapshot, y las métricas de stage quedan fuera del informe.
        eprintln!(
            "Aviso: no hay archivos modificados; se omiten las métricas de stage. \
             Usa --stage-file <ruta-relativa> para forzar una."
        );
    }
    candidate
}

fn run_benchmark(
    repository: &Path,
    iterations: usize,
    stage_relative: Option<&Path>,
) -> BenchReport {
    let client = GitClient::default();
    let cancellation = CancellationToken::default();
    let _ = client.detect_version(&cancellation);

    for _ in 0..WARMUP_ITERATIONS {
        let _ = client.status(repository, &cancellation);
        let _ = client.snapshot(repository, &cancellation);
    }

    let spawn_cost_ms = collect_samples(iterations, || {
        measure(|| {
            let _ = client.detect_version(&cancellation).unwrap();
        })
    });
    let status_ms = collect_samples(iterations, || {
        measure(|| {
            let _ = client.status(repository, &cancellation).unwrap();
        })
    });
    let snapshot_ms = collect_samples(iterations, || {
        measure(|| {
            let _ = client.snapshot(repository, &cancellation).unwrap();
        })
    });
    let snapshot_with_history_ms = collect_samples(iterations, || {
        measure(|| {
            let _ = client
                .snapshot_with_history(repository, 200, &cancellation)
                .unwrap();
        })
    });
    let stage = stage_relative.map(|stage_relative| {
        let stage_flow = collect_samples(iterations, || {
            measure(|| {
                client
                    .stage(repository, stage_relative, &cancellation)
                    .unwrap();
                client.snapshot(repository, &cancellation).unwrap();
                client
                    .unstage(repository, stage_relative, &cancellation)
                    .unwrap();
            })
        });
        let stage_only = collect_samples(iterations, || {
            measure(|| {
                client
                    .stage(repository, stage_relative, &cancellation)
                    .unwrap();
                client
                    .unstage(repository, stage_relative, &cancellation)
                    .unwrap();
            })
        });
        let git_cli_stage = collect_samples(iterations, || {
            measure(|| run_git_cli_stage(repository, stage_relative))
        });
        // El sobrecoste compara el mismo trabajo en ambos lados (stage + unstage). `stage_flow`
        // incluye además el snapshot de refresco, que el Git CLI no hace, y no es comparable.
        let stage_overhead = diff_percentiles(&stage_only, &git_cli_stage);
        StageMeasurements {
            stage_flow,
            stage_only,
            git_cli_stage,
            stage_overhead,
        }
    });

    BenchReport {
        metadata: Metadata {
            repository: repository.display().to_string(),
            iterations,
            warmup_iterations: WARMUP_ITERATIONS,
            build_profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            git_helper_version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: std::env::consts::OS.to_owned(),
            recorded_at: chrono_lite_timestamp(),
        },
        measurements: Measurements {
            spawn_cost: spawn_cost_ms,
            status: status_ms,
            snapshot: snapshot_ms,
            snapshot_with_history: snapshot_with_history_ms,
            stage,
        },
    }
}

fn emit_report(report: &BenchReport, json_output: bool) {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(report).expect("serialización JSON")
        );
    } else {
        print_human_report(report);
    }
}

fn discover_stage_candidate(repository: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        // `--untracked-files=all` evita que un directorio sin seguir se colapse en una sola
        // entrada: sin esto el candidato podía ser una carpeta con miles de archivos y la
        // medición de "stage de un archivo" no medía eso en absoluto.
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter_map(parse_porcelain_path)
        .map(PathBuf::from)
        .find(|candidate| repository.join(candidate).is_file())
}

/// Extrae la ruta de una línea `git status --porcelain` (v1).
///
/// Descarta rutas entrecomilladas (`core.quotepath` las escapa y no se pueden usar tal cual)
/// y se queda con el destino en los renombrados `XY origen -> destino`.
fn parse_porcelain_path(line: &str) -> Option<&str> {
    let path = line.get(3..)?.trim();
    let path = path.rsplit(" -> ").next()?;
    if path.is_empty() || path.starts_with('"') {
        return None;
    }
    Some(path)
}

fn run_git_cli_stage(repository: &Path, relative_path: &Path) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["add", "--"])
        .arg(relative_path)
        .status()
        .expect("git add debe ejecutarse");
    assert!(status.success(), "git add falló");
    let status = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["restore", "--staged", "--"])
        .arg(relative_path)
        .status()
        .expect("git restore --staged debe ejecutarse");
    assert!(status.success(), "git restore --staged falló");
}

fn measure(operation: impl FnOnce()) -> f64 {
    let start = Instant::now();
    operation();
    start.elapsed().as_secs_f64() * 1000.0
}

fn collect_samples(iterations: usize, mut operation: impl FnMut() -> f64) -> PercentileStats {
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        samples.push(operation());
    }
    summarize(&samples)
}

fn summarize(samples: &[f64]) -> PercentileStats {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let len = sorted.len();
    let sample_count = f64::from(u32::try_from(len).unwrap_or(u32::MAX));
    PercentileStats {
        samples: len,
        minimum: sorted[0],
        median: percentile(&sorted, 50),
        tail: percentile(&sorted, 95),
        maximum: sorted[len - 1],
        average: sorted.iter().sum::<f64>() / sample_count,
    }
}

fn percentile(sorted: &[f64], rank_percent: u8) -> f64 {
    let len = sorted.len();
    if len <= 1 {
        return sorted[0];
    }
    let index = (len - 1) * usize::from(rank_percent) / 100;
    sorted[index]
}

fn diff_percentiles(app: &PercentileStats, baseline: &PercentileStats) -> PercentileStats {
    PercentileStats {
        samples: app.samples.min(baseline.samples),
        minimum: app.minimum - baseline.minimum,
        median: app.median - baseline.median,
        tail: app.tail - baseline.tail,
        maximum: app.maximum - baseline.maximum,
        average: app.average - baseline.average,
    }
}

fn print_human_report(report: &BenchReport) {
    println!("Git Helper bench v{}", report.metadata.git_helper_version);
    println!("Repositorio: {}", report.metadata.repository);
    println!(
        "Perfil: {} · Plataforma: {} · Iteraciones: {} (+{} calentamiento)",
        report.metadata.build_profile,
        report.metadata.platform,
        report.metadata.iterations,
        report.metadata.warmup_iterations
    );
    print_stats("spawn (git --version)", &report.measurements.spawn_cost);
    print_stats("status", &report.measurements.status);
    print_stats("snapshot (sin historial)", &report.measurements.snapshot);
    print_stats(
        "snapshot + historial(200)",
        &report.measurements.snapshot_with_history,
    );
    let Some(stage) = &report.measurements.stage else {
        println!("  stage: sin cambios en el repositorio, métricas omitidas");
        return;
    };
    print_stats("stage + snapshot + unstage (app)", &stage.stage_flow);
    print_stats("stage + unstage (app)", &stage.stage_only);
    print_stats("stage + unstage (git CLI)", &stage.git_cli_stage);
    print_stats("sobrecoste app vs CLI (p95)", &stage.stage_overhead);
}

fn print_stats(label: &str, stats: &PercentileStats) {
    println!(
        "  {label:<32} min={:.1} p50={:.1} p95={:.1} max={:.1} mean={:.1} ms",
        stats.minimum, stats.median, stats.tail, stats.maximum, stats.average
    );
}

fn chrono_lite_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    format!("unix:{seconds}")
}
