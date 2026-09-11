# Decisiones técnicas

## Bootstrap de GPUI

| Elemento | Valor |
|---|---|
| Repositorio | `zed-industries/zed` |
| Revisión fijada | `5896274168e06663568f42b5bae142162936e0a6` |
| Toolchain de Rust | `1.97.1` |
| Target inicial | `x86_64-pc-windows-msvc` |
| Crates usados | `gpui`, `gpui_platform` |

La revisión corresponde al `HEAD` de Zed consultado durante el bootstrap y utiliza la misma versión
de Rust declarada por ese commit. `Cargo.toml` fija el SHA completo para impedir cambios
involuntarios de API.

Se han consultado los ejemplos de GPUI y el panel Git de Zed en la misma revisión. Git Helper no
depende de los crates GPL `git_ui` o `git`: la ejecución de Git, los modelos y los parsers son
implementaciones propias sobre `git.exe`.

Git Helper se publica bajo Apache-2.0, compatible con GPUI y con el código Apache-2.0 reutilizado de
los ejemplos de Zed. No se copió código GPL de Zed.

## Trazabilidad con Zed

| Origen en Zed | Módulo de Git Helper | Adaptación |
|---|---|---|
| `crates/git_ui/src/git_panel.rs` | `src/ui/main_window.rs` | Se mantienen pestañas internas, lista compacta, formulario inferior, barra de rama y tareas GPUI. Se eliminan `Workspace`, `Project`, editor, diffs, telemetría y colaboración. No se copia código GPL. |
| `crates/git_ui/src/commit_message_prompt.txt` | `src/cursor/context_builder.rs` | Se conserva la intención del prompt, añadiendo tratamiento del diff como datos no confiables, contexto staged estructurado y límite de 200 KiB. Redacción propia. |
| `crates/git_ui/src/commit_view.rs` | `src/ui/main_window.rs` | Historial cronológico virtualizado sin diff ni git graph. |
| `crates/git_ui/src/remote_output.rs` | `src/git/client.rs` | Los remotes se ejecutan como procesos cancelables; stderr se conserva en errores tipados. |
| `crates/git_ui/src/repository_selector.rs` | pestañas de `src/ui/main_window.rs` | Una sesión persistente por raíz canónica en lugar de un repositorio activo del workspace. |
| `crates/git/src/status.rs` | `src/domain/status.rs`, `src/git/status_parser.rs` | Modelo reducido al MVP y parser propio de porcelain v2 delimitado por NUL. |
| `crates/git/src/commit.rs` | `src/domain/history.rs`, `src/git/log_parser.rs` | Formato propio de doce campos NUL y lista de referencias, sin lanes. |
| `crates/git/src/remote.rs` | `src/domain/remote.rs`, `src/git/remote_plan.rs` | Planes tipados que excluyen push forzado y pull no fast-forward. |
| `crates/gpui/examples/uniform_list.rs` (Apache-2.0) | listas de `src/ui/main_window.rs` | Uso de la API pública `uniform_list` con processors ligados a la entidad. |
| `crates/gpui/examples/input.rs` (Apache-2.0) | `src/ui/commit_input.rs` | Implementación reducida y modificada de `EntityInputHandler`; admite texto multilínea y elimina la lógica de layout/caret del ejemplo. |

## Límites de procesos

`src/process.rs` es la única abstracción de procesos. No usa una shell, conserva argumentos como
`OsString` y aplica timeouts. stdout, stderr y stdin se redirigen a temporales anónimos: así se
mantiene la captura independiente de ambos streams sin crear lectores bloqueables cuando un
descendiente hereda los handles. En Windows, la cancelación y el timeout finalizan el árbol activo
con `taskkill.exe /PID <pid> /T /F`; el comando auxiliar también se crea con `CREATE_NO_WINDOW`, no
usa shell y tiene un límite de cleanup de dos segundos. Si `taskkill.exe` falla —lo hace también cuando el hijo
acaba de terminar por su cuenta— se registra el aviso y se continúa con el hijo directo; el runner
solo devuelve un error de infraestructura si el proceso sigue vivo tras la espera acotada, de modo
que la clasificación de cancelación o timeout nunca se pierde por esa carrera. La salida normal del padre no espera a
descendientes que se hayan desacoplado voluntariamente; los datos capturados se leen sin esperar al
cierre de sus handles. Git recibe `GIT_TERMINAL_PROMPT=0`; Cursor CLI no hereda `CURSOR_API_KEY` ni
`CURSOR_API_TOKEN`.

Las pruebas de proceso cubren captura, timeout y cancelación con una jerarquía Windows que hereda
los handles de salida, además de la salida normal de un padre cuyo descendiente sigue activo. El
descendiente es el propio binario de pruebas —no un intérprete externo, cuyo arranque decidía en CI
si la prueba llegaba a comprobar algo—, publica su PID y las pruebas verifican su desaparición con
`tasklist.exe`; no se usa la ausencia de un archivo como prueba de terminación, porque sería cierta
antes incluso de que el descendiente pudiera escribirlo. La
comprobación funcional de Windows debe ejecutarse en build release porque el entorno de desarrollo
puede no tener Cargo o Windows disponible.

## Inventario de ramas e historial

Las ramas locales y referencias remote-tracking se obtienen con una única lectura NUL-delimitada
de `git for-each-ref` y se guardan en una caché por repositorio. La caché se invalida cuando cambian
las refs o la configuración Git; no se lanza un proceso por rama ni se consulta la red para mostrar
referencias remotas. `refs/remotes/*/HEAD` se descarta por ser simbólica.

El historial seleccionado se resuelve a un OID y se consulta con `git log <oid> --`, por lo que
seleccionar una rama no hace checkout ni modifica `HEAD`, el índice o el working tree. Cada carga
paginada conserva la referencia, el OID, el repositorio y una generación de sesión; los resultados
que llegan tarde después de cambiar de rama o de pestaña se descartan.

## Generación segura de mensajes de commit

Cada propuesta de Cursor queda asociada a la sesión, a un identificador de solicitud y a la versión
del borrador visible. Antes de aplicar la respuesta se comprueba que la solicitud siga activa, que
el usuario no haya editado el borrador y que la identidad de `git diff --cached --raw -z` coincida
con la que originó el prompt. Así, cambiar el contenido staged de un archivo invalida la propuesta
aunque no cambien sus nombres ni el número de archivos; los cambios exclusivamente unstaged no la
invalidan. Las respuestas obsoletas, canceladas o de pestañas cerradas se descartan sin tocar el
texto actual. La generación solo propone texto: nunca hace stage ni commit.

## Watcher y prioridad de refresco

El watcher mantiene una cola acotada de 256 eventos. Una ráfaga se agrupa con un debounce trailing
de 250 ms y una espera máxima de 2 s desde el primer evento; si la cola se desborda, se solicita
una reconciliación completa en lugar de intentar conservar cada evento individual. Esto evita que
un árbol generado por una compilación haga crecer la memoria o posponga indefinidamente la
actualización.

Solo el repositorio visible se refresca automáticamente. Las pestañas inactivas conservan una
marca de actualización pendiente y se reconcilian al seleccionarlas, evitando que diez
repositorios compitan por procesos Git mientras el usuario trabaja en uno. Al recuperar el foco se
fuerza la misma reconciliación del repositorio activo.

Se observan el working tree, el git-dir de la sesión y el git-common-dir cuando el repositorio es un
worktree vinculado. Las reglas ignoradas se vuelven a consultar al cambiar `.gitignore` o
`.git/info/exclude` y al aparecer un directorio nuevo: como máximo el primer lote atraviesa el
filtro y el watcher se reconstruye con las rutas ignoradas actuales, sin ejecutar Git por evento.
Si notify comunica un error, la UI conserva el estado visible, retira el watcher fallido y deja F5
como recuperación explícita; un refresh correcto vuelve a instalar la vigilancia.

## Coordinación de refrescos (UX-01)

Cada `RepositorySession` incluye un `RefreshCoordinator` con dos banderas: `in_flight` indica si hay
una lectura de estado en curso y `dirty` acumula invalidaciones recibidas mientras tanto. Una
solicitud de refresh solo arranca un proceso Git cuando `request()` devuelve `true`; las peticiones
concurrentes marcan `dirty` y se encolan sin crear una tarea por evento.

Al terminar un refresh —con éxito, error o cancelación— `finish()` libera `in_flight` y devuelve si
hace falta como máximo un refresh adicional que consuma lo pendiente. Si llegan eventos durante ese
segundo refresh, vuelven a marcar `dirty` y el ciclo se repite una vez más. Las lecturas de status
siguen usando `GIT_OPTIONAL_LOCKS=0` para no reactivar el watcher por cambios en `.git/index`.

Las mutaciones Git (stage, unstage, descarte, commit, fetch, pull, push) y la generación de mensaje
con Cursor se serializan por repositorio: mientras `mutation_state` está en `Running`, los refreshes
solicitados marcan `dirty` y se guardan en `pending_refreshes`. Al finalizar la mutación —incluso si
falla o se cancela— se llama a `refresh_repository` para reconciliar el snapshot con el working tree
real.

Al cerrar una pestaña se cancelan los tokens activos, se eliminan watchers y se descartan respuestas
cuya generación ya no coincide con la sesión. Un watcher que termine de instalarse después del cierre
no se registra ni procesa eventos.

## Persistencia

El esquema actual es la versión 1. `state.json` se escribe mediante un archivo temporal sincronizado
y reemplazo atómico. Un JSON corrupto se mueve a `state.corrupt-<timestamp>.json` y el arranque
continúa con estado vacío.

## Working tree e historial desacoplados (PERF-03)

Cada `RepositorySession` separa `working_tree` (`Arc<WorkingTreeSnapshot>`) e `history`
(`Arc<HistorySnapshot>`). Un refresh de lectura solo compara y sustituye el working tree; el
historial paginado permanece intacto salvo invalidación explícita (cambio de `HEAD`/upstream,
selección de otra rama o carga diferida). Los commits se almacenan en `Arc<Vec<CommitSummary>>`
para que `render_history` y la paginación no clonen miles de filas en cada frame.

Los contadores de la pestaña Cambios (`change_count`, `staged_count`) se derivan una vez al
actualizar el working tree. Los detalles de commit se cachean por repositorio con un límite fijo
(32 entradas, LRU) para evitar clonados profundos al alternar selección.

La medición manual ignorada en la prueba unitaria
(`finish_refresh_comparison_cost_is_bounded_with_large_history`) conserva el umbral de referencia:
con 10 000 commits cargados, `finish_refresh` tras un cambio del working tree completa en menos de
50 ms porque ya no recorre ni compara la lista de commits. La garantía de regresión que se ejecuta
en CI es determinista y comprueba que los `Arc` del historial permanecen intactos.

## Estados de interacción por repositorio

Cada `RepositorySession` mantiene por separado `refresh_state` y `mutation_state`. El refresh
puede conservar el snapshot anterior mientras carga, y sus transiciones se notifican aunque Git
devuelva exactamente los mismos datos. Las mutaciones se serializan por repositorio y no utilizan
el estado de otra pestaña para bloquearse ni para mostrar errores. La sesión conserva también el
último `status_message` y el error accionable; los errores globales quedan reservados para fallos
de la aplicación, como persistencia, selección de carpeta o detección de Git. Una cancelación usa
un estado distinto de un fallo para que la UI no la presente como error.
