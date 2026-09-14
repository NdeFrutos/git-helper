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
- Clonar repositorios remotos accesibles por SSH y abrirlos en una pestaña.
- Ver cambios staged, sin preparar, sin seguimiento y en conflicto.
- Hacer stage/unstage por archivo o en bloque y descartar cambios con confirmación.
- Crear commits sin añadir archivos automáticamente.
- Ejecutar `fetch`, `pull --ff-only` y `push`, incluida la primera publicación de una rama.
- Ver la antigüedad de las referencias remotas y activar fetch automático (desactivado por defecto; Shift+clic alterna 5/15/30 min).
- Consultar ramas locales y referencias remote-tracking, con upstream y contadores ahead/behind.
- Consultar el historial y los detalles de cada commit.
- Restaurar los repositorios abiertos y la vista seleccionada entre sesiones.
- Proponer un mensaje de commit con Cursor CLI, de forma opcional y siempre editable.
- Configurar idioma, convención, alcance y longitud del asunto del mensaje, con valores
  globales y sobrescritura por repositorio, e insertar una plantilla manual editable.

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

Durante la instalación con MSI puedes marcar la feature opcional **PATH Environment Variable**
para invocar `ghelper` desde cualquier terminal. En el ZIP portable, añade manualmente la carpeta
`bin` (donde están `git-helper.exe` y `ghelper.exe`) al `PATH` del sistema o del usuario si quieres
el mismo acceso por línea de comandos.

### Requisitos de uso

- Windows 10 u 11 de 64 bits.
- [Git for Windows](https://gitforwindows.org/).
- Opcional: [Cursor CLI](https://cursor.com/cli) y una sesión iniciada para generar mensajes.
- Para clonar por SSH: clave en `ssh-agent` (`ssh-add`), entrada en `~/.ssh/known_hosts` y Git for Windows con su SSH habitual.

## Línea de comandos

Con la feature PATH del MSI (o la carpeta `bin` en el PATH del ZIP portable) puedes abrir
repositorios desde PowerShell, Cursor o T3 Code:

```powershell
ghelper --help
ghelper .
ghelper "C:\Users\dev\proyectos\mi-repo"
```

- `ghelper [RUTA]` abre Git Helper con ese repositorio en la pestaña activa. La ruta puede ser
  absoluta, relativa al directorio actual o `.` estando ya dentro del repo.
- Sin argumentos restaura la sesión persistida, igual que abrir la aplicación desde el menú Inicio.
- Si Git Helper ya está en ejecución, la solicitud se reenvía a la ventana existente: se activa la
  pestaña del repositorio o se abre una nueva si aún no estaba cargado.
- Rutas inexistentes, carpetas que no son repositorios Git o Git ausente del PATH muestran un error
  en la consola y devuelven un código de salida distinto de cero, sin dejar procesos huérfanos.

`ghelper.exe` es un launcher de consola; `git-helper.exe` sigue siendo la aplicación gráfica que
aparece en el menú Inicio.

## Uso rápido

1. Abre Git Helper y selecciona un repositorio con `Abrir repositorio` (`Ctrl+O`) o clónalo con `Clonar repositorio` (`Ctrl+Shift+O`).
2. Prepara los archivos que quieras incluir desde la vista `Cambios`.
3. Escribe el mensaje —o solicita una propuesta a Cursor— y pulsa `Commit`.
4. Usa `Fetch`, `Pull` o `Push` desde la barra superior cuando necesites sincronizar.

### Preferencias del mensaje de commit

Bajo el cuadro de mensaje hay una fila compacta con las preferencias que orientan la propuesta:

| Control | Qué hace |
|---|---|
| `Editando: global` / `Editando: este repo` | Elige la capa sobre la que actúan los botones siguientes |
| `Idioma` | Recorre `según el historial`, `español` e `inglés` |
| `Formato` | Alterna entre `texto libre` y `conventional` |
| `alcance opcional` | Solo con `conventional`: recorre `sin alcance`, `alcance opcional` y `alcance obligatorio` |
| `Asunto ≤N` | Recorre las longitudes orientativas 50, 60, 72 y 100 |
| `Usar global` | Elimina las sobrescrituras del repositorio activo |
| `Predeterminados` | Restaura los valores de fábrica globales y del repositorio activo |
| `Plantilla` | Escribe una plantilla editable acorde a las preferencias efectivas |

Un valor marcado con `·repo` procede del repositorio activo; el resto se hereda del ajuste global.
Cualquier proveedor de generación recibe exactamente las mismas preferencias normalizadas.

Las convenciones son una guía: el aviso naranja bajo el cuadro señala cuándo el asunto se aleja de
lo configurado, pero nunca impide crear un commit escrito a mano. `Plantilla` tampoco sustituye un
borrador con texto sin confirmarlo antes, y una propuesta generada con preferencias distintas a las
vigentes se descarta en lugar de pisar el borrador.

Atajos disponibles:

| Atajo | Acción |
|---|---|
| `Ctrl+O` | Abrir repositorio |
| `Ctrl+Shift+O` | Clonar repositorio por SSH |
| `Ctrl+W` | Cerrar la pestaña activa |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Cambiar de repositorio |
| `F5` | Actualizar el estado |
| `Ctrl+1` / `Ctrl+2` | Mostrar historial / cambios |
| `Ctrl+Enter` | Crear el commit |
| `Ctrl+Shift+G` | Generar un mensaje con Cursor |

## Cuando algo falla

La banda de error muestra tres cosas: qué ocurrió, el siguiente paso seguro y los detalles
técnicos, que se pueden expandir y copiar. Los casos reconocidos —identidad de Git sin configurar,
`index.lock` bloqueado, hook que rechaza la operación, credenciales rechazadas, rama sin upstream,
push rechazado, divergencia con el remote y fallos del proveedor de IA— incluyen una recomendación
concreta. Un error que Git Helper no reconoce conserva su salida íntegra y no se le atribuye una
causa inventada.

Las recomendaciones nunca reparan nada por su cuenta: Git Helper no borra `index.lock`, no cambia
tu configuración de identidad, no hace merge, rebase ni stash automático y nunca usa push forzado.
Las URLs con credenciales embebidas se ocultan antes de mostrar o copiar los detalles.

## Privacidad y seguridad

- No hay telemetría propia.
- Git y Cursor CLI se ejecutan directamente, sin `cmd.exe` ni PowerShell y con argumentos
  separados.
- Los mensajes de commit se entregan a Git por `stdin` y los hooks se respetan.
- `pull` siempre usa `--ff-only` y `push` nunca usa opciones de fuerza.
- Descartar cambios requiere confirmación explícita.
- Cursor solo recibe el contexto staged tras una acción y consentimiento explícitos. El contexto
  está limitado a 200 KiB, excluye binarios y no se guarda ni se registra.
- Las preferencias del mensaje se guardan en el estado local de la aplicación; no se leen
  instrucciones del repositorio ni se envía nada más que el contexto staged.
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
La comprobación ligera de rendimiento del panel está en
[docs/performance-check.md](docs/performance-check.md).

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
| `src/process.rs` | Procesos sin shell, captura aislada, cancelación y timeout |
| `src/ui/` | Ventana, entrada de commit, listas virtualizadas y tema GPUI |
| `src/watcher/` | Observación del repositorio y debounce de eventos |

La aplicación está escrita en Rust con
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), fijado a un commit concreto de
Zed. La especificación original está en [SPEC.md](SPEC.md), y la justificación de las decisiones y
la trazabilidad de código están en [docs/technical-decisions.md](docs/technical-decisions.md).

## Limitaciones conocidas

- El soporte oficial se limita a Windows 10/11 x64.
- El clonado remoto admite URLs SSH; HTTPS y otros esquemas quedan pendientes.
- Git Helper no gestiona claves SSH: usa `ssh-agent`, `~/.ssh/config` y el SSH incluido en Git for Windows.
- No incluye diff, editor, git graph, checkout de ramas, stash, rebase ni resolución visual de
  conflictos.
- La vista de ramas es informativa: consultar otra rama carga su historial por referencia sin
  modificar `HEAD`, el índice ni el working tree. Las referencias remotas reflejan el último
  `fetch` y no consultan la red por sí mismas. La barra de acciones indica la frescura del remote
  principal; `Actualizar estado` solo relee refs locales. El fetch automático está desactivado por
  defecto, respeta un intervalo configurable (5 minutos por defecto) y se pospone durante mutaciones
  o generación de mensajes.
- Si existen varios remotes y no hay upstream, algunas operaciones necesitan una selección
  explícita; esa elección se recuerda para fetch manual y automático.
- El instalador aún no está firmado digitalmente.
- GPUI todavía es pre-1.0 y puede exigir cambios al actualizar su revisión.

Git Helper se distribuye bajo [Apache License 2.0](LICENSE). Las atribuciones y licencias de sus
dependencias están en [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) y
[`docs/dependency-licenses.txt`](docs/dependency-licenses.txt).
