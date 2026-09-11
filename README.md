# Git Helper

Git Helper es una aplicación de escritorio nativa para Windows que concentra el flujo Git habitual
de varios repositorios en una sola ventana: revisar cambios, preparar archivos, hacer commits,
sincronizar con remotes y consultar el historial.

[![Release](https://img.shields.io/github/v/release/NdeFrutos/git-helper?display_name=tag)](https://github.com/NdeFrutos/git-helper/releases/latest)
[![Licencia](https://img.shields.io/github/license/NdeFrutos/git-helper)](LICENSE)

> Este es un proyecto *vibecodeado*: nació para aislar y explorar esta funcionalidad fuera de un
> editor completo. El código, las decisiones y las limitaciones se mantienen visibles para que el
> experimento sea auditable y útil por sí mismo.

## Qué permite hacer

- Abrir varios repositorios locales y cambiar entre ellos mediante pestañas.
- Ver cambios staged, sin preparar, sin seguimiento y en conflicto.
- Hacer stage/unstage por archivo o en bloque y descartar cambios con confirmación.
- Crear commits sin añadir archivos automáticamente.
- Ejecutar `fetch`, `pull --ff-only` y `push`, incluida la primera publicación de una rama.
- Consultar ramas locales y referencias remote-tracking, con upstream y contadores ahead/behind.
- Consultar el historial y los detalles de cada commit.
- Restaurar los repositorios abiertos y la vista seleccionada entre sesiones.
- Proponer un mensaje de commit con Cursor CLI, de forma opcional y siempre editable.

Git Helper usa el Git instalado en el equipo, por lo que respeta sus credenciales, configuración,
hooks, filtros y atributos. No incluye un motor Git propio.

## Instalación

### Instalador MSI

1. Abre la [última release](https://github.com/NdeFrutos/git-helper/releases/latest).
2. Descarga el archivo `git-helper-<versión>-windows-x86_64.msi`.
3. Comprueba, si lo deseas, su huella con el archivo `SHA256SUMS.txt` de la misma release.
4. Ejecuta el MSI, sigue el asistente y abre `Git Helper` desde el menú Inicio.

El instalador todavía no está firmado con Authenticode, por lo que Windows puede mostrar una
advertencia de editor desconocido. Verifica que la descarga procede de este repositorio y que su
SHA-256 coincide antes de continuar. La instalación es para todo el equipo y puede solicitar
permisos de administrador.

También se publica `git-helper-<versión>-windows-x86_64-portable.zip`: basta con extraerlo y abrir
`git-helper.exe`. Git for Windows debe estar disponible en `PATH` en ambos casos.

### Requisitos de uso

- Windows 10 u 11 de 64 bits.
- [Git for Windows](https://gitforwindows.org/).
- Opcional: [Cursor CLI](https://cursor.com/cli) y una sesión iniciada para generar mensajes.

## Uso rápido

1. Abre Git Helper y selecciona un repositorio con `Abrir repositorio` o `Ctrl+O`.
2. Prepara los archivos que quieras incluir desde la vista `Cambios`.
3. Escribe el mensaje —o solicita una propuesta a Cursor— y pulsa `Commit`.
4. Usa `Fetch`, `Pull` o `Push` desde la barra superior cuando necesites sincronizar.

Atajos disponibles:

| Atajo | Acción |
|---|---|
| `Ctrl+O` | Abrir repositorio |
| `Ctrl+W` | Cerrar la pestaña activa |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Cambiar de repositorio |
| `F5` | Actualizar el estado |
| `Ctrl+1` / `Ctrl+2` | Mostrar historial / cambios |
| `Ctrl+Enter` | Crear el commit |
| `Ctrl+Shift+G` | Generar un mensaje con Cursor |

## Privacidad y seguridad

- No hay telemetría propia.
- Git y Cursor CLI se ejecutan directamente, sin `cmd.exe` ni PowerShell y con argumentos
  separados.
- Los mensajes de commit se entregan a Git por `stdin` y los hooks se respetan.
- `pull` siempre usa `--ff-only` y `push` nunca usa opciones de fuerza.
- Descartar cambios requiere confirmación explícita.
- Cursor solo recibe el contexto staged tras una acción y consentimiento explícitos. El contexto
  está limitado a 200 KiB, excluye binarios y no se guarda ni se registra.
- La detección básica de posibles secretos antes de usar Cursor es una ayuda, no una garantía.

## Compilar desde el código fuente

Se necesita Windows 10/11 x64, Git, Rust 1.97.1 y Visual Studio Build Tools 2022 con el workload
`Desarrollo para el escritorio con C++`, MSVC v143 y Windows SDK.

```powershell
git clone https://github.com/NdeFrutos/git-helper.git
cd git-helper
cargo run
```

`rust-toolchain.toml` hace que `rustup` instale la versión y los componentes correctos. Para validar
un cambio local:

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
```

La generación reproducible del MSI se documenta en [docs/packaging.md](docs/packaging.md).

## Releases

El workflow [`.github/workflows/release.yml`](.github/workflows/release.yml) valida la versión en
`preflight`, ejecuta checks y empaquetado en paralelo y publica solo cuando ambos jobs aprueban el
mismo SHA. Los artefactos (MSI, ZIP portable y checksums) se transfieren como artefacto de Actions;
`publish` no recompila. Hay dos formas de iniciarlo:

- Crear y subir una etiqueta que coincida con la versión de `Cargo.toml`, por ejemplo
  `git tag v0.1.0 && git push origin v0.1.0`.
- Ejecutar manualmente `Release` desde GitHub Actions e indicar esa misma versión sin la `v`.

La ejecución manual admite `dry_run` (valida sin crear release) y `failure_mode=checks|package` para
comprobar que un fallo bloquea `publish`. Detalles, grafo de dependencias y medición de tiempos en
[docs/packaging.md](docs/packaging.md).

Si la etiqueta y `Cargo.toml` no coinciden, el workflow se detiene en `preflight`, antes de los jobs
costosos. Para preparar una nueva versión, actualiza `Cargo.toml` y `Cargo.lock`, integra el cambio
en `main` y lanza el workflow. Las notas de release se generan automáticamente a partir del historial
de GitHub.

## Arquitectura

| Ruta | Responsabilidad |
|---|---|
| `src/domain/` | Modelo de repositorios, cambios, historial y remotes |
| `src/git/` | Única puerta de acceso a `git.exe`, parsers y planes seguros |
| `src/cursor/` | Contexto staged limitado, ejecución de `agent` y parser JSON |
| `src/persistence/` | Estado versionado y escritura atómica |
| `src/process.rs` | Procesos sin shell, pipes, cancelación y timeout |
| `src/ui/` | Ventana, entrada de commit, listas virtualizadas y tema GPUI |
| `src/watcher/` | Observación del repositorio y debounce de eventos |

La aplicación está escrita en Rust con
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), fijado a un commit concreto de
Zed. La especificación original está en [SPEC.md](SPEC.md), y la justificación de las decisiones y
la trazabilidad de código están en [docs/technical-decisions.md](docs/technical-decisions.md).

## Limitaciones conocidas

- El soporte oficial se limita a Windows 10/11 x64.
- No incluye diff, editor, git graph, checkout de ramas, stash, rebase ni resolución visual de
  conflictos.
- La vista de ramas es informativa: consultar otra rama carga su historial por referencia sin
  modificar `HEAD`, el índice ni el working tree. Las referencias remotas reflejan el último
  `fetch` y no consultan la red por sí mismas.
- Si existen varios remotes y no hay upstream, algunas operaciones necesitan una selección
  explícita.
- El instalador aún no está firmado digitalmente.
- GPUI todavía es pre-1.0 y puede exigir cambios al actualizar su revisión.

Git Helper se distribuye bajo [Apache License 2.0](LICENSE). Las atribuciones y licencias de sus
dependencias están en [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) y
[`docs/dependency-licenses.txt`](docs/dependency-licenses.txt).
