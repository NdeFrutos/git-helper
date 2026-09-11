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

## Inventario de ramas e historial

Las ramas locales y referencias remote-tracking se obtienen con una única lectura NUL-delimitada
de `git for-each-ref` y se guardan en una caché por repositorio. La caché se invalida cuando cambian
las refs o la configuración Git; no se lanza un proceso por rama ni se consulta la red para mostrar
referencias remotas. `refs/remotes/*/HEAD` se descarta por ser simbólica.

El historial seleccionado se resuelve a un OID y se consulta con `git log <oid> --`, por lo que
seleccionar una rama no hace checkout ni modifica `HEAD`, el índice o el working tree. Cada carga
paginada conserva la referencia, el OID, el repositorio y una generación de sesión; los resultados
que llegan tarde después de cambiar de rama o de pestaña se descartan.

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
