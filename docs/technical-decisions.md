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
`OsString`, lee stdout y stderr en paralelo, permite cancelación cooperativa y aplica timeouts. Git
recibe `GIT_TERMINAL_PROMPT=0`; Cursor CLI no hereda `CURSOR_API_KEY` ni
`CURSOR_API_TOKEN`. En Windows, todos los procesos hijos se crean con `CREATE_NO_WINDOW` para que
las operaciones en segundo plano no abran consolas sobre la interfaz gráfica.

## Lotes de rutas para stage y unstage

Actuar sobre una selección envía varios pathspecs al mismo comando. `src/git/path.rs` valida el
lote completo antes de ejecutar nada —un pathspec inseguro lo aborta sin lanzar ningún proceso— y
lo reparte con `plan_pathspec_batches` en invocaciones que caben en `CreateProcessW`, cuyo límite
son 32 767 unidades UTF-16. Se reserva presupuesto para el prefijo fijo (`git -C <raíz> add --`) y
se aplican dos cotas: `COMMAND_LINE_BUDGET = 30 000` unidades, que deja margen para el
entrecomillado que añade el runtime, y `MAX_PATHS_PER_BATCH = 512`, para no construir procesos con
miles de argumentos. El orden de las rutas se conserva.

Se prefirió el troceado a `--pathspec-from-file=- --pathspec-file-nul` para no depender de Git
2.25 o superior; si en el futuro se fija una versión mínima, esa opción elimina el troceado entero.

Git no ofrece atomicidad entre rutas y la aplicación no la finge. Cuando Git **rechaza** un lote no
indica qué ruta lo provocó, así que ese lote se repite ruta a ruta y el resultado se informa como
`GitError::PartialBatch` con las rutas aplicadas, las pedidas y el motivo de cada fallo. Un fallo
del proceso —cancelación, timeout o `git.exe` no disponible— afecta a todo el lote y **no** se
reintenta ruta a ruta: hacerlo multiplicaría la espera y el bloqueo del repositorio sin aportar
información. Después de cualquier resultado se reconcilia el estado con Git.

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

## Persistencia

El esquema actual es la versión 1. `state.json` se escribe mediante un archivo temporal sincronizado
y reemplazo atómico. Un JSON corrupto se mueve a `state.corrupt-<timestamp>.json` y el arranque
continúa con estado vacío.

## Estados de interacción por repositorio

Cada `RepositorySession` mantiene por separado `refresh_state` y `mutation_state`. El refresh
puede conservar el snapshot anterior mientras carga, y sus transiciones se notifican aunque Git
devuelva exactamente los mismos datos. Las mutaciones se serializan por repositorio y no utilizan
el estado de otra pestaña para bloquearse ni para mostrar errores. La sesión conserva también el
último `status_message` y el error accionable; los errores globales quedan reservados para fallos
de la aplicación, como persistencia, selección de carpeta o detección de Git. Una cancelación usa
un estado distinto de un fallo para que la UI no la presente como error.
