# Auditoría de rendimiento — Git Helper

Fecha: 2026-09-10 · Rama: `main` @ `714a7ba` · Autor: auditoría automatizada
Estado: **informe solamente, no se ha modificado ni una línea de código de la aplicación.**

---

## 1. Veredicto

El problema **no es Rust ni GPUI**. Es que la aplicación gasta casi todo su tiempo esperando a
procesos `git.exe`, y los lanza de forma secuencial, redundante y en bucle.

Medición directa sobre este propio repositorio: **una sola actualización de estado
(`GitClient::snapshot`) tarda entre 563 ms y 2 078 ms.** Un clic en "Stage" sobre un único archivo
desencadena ~10 procesos `git.exe` y aproximadamente **1,3–1,5 s** antes de que la fila se
actualice. Y como el watcher se autoalimenta, buena parte de ese trabajo se repite sin que nadie lo
haya pedido.

Los tres focos, por orden de impacto medido:

| # | Causa | Coste medido | Fichero |
|---|---|---|---|
| 1 | `snapshot()` lanza 4–5 procesos git **en serie**, y siempre incluye el historial que la vista activa no usa | 563–2 078 ms por refresco | `src/git/client.rs:118-121` |
| 2 | El watcher se **autoalimenta**: el `git status` del refresco reescribe `.git/index`, que el watcher vigila | duplica cada refresco | `src/watcher/repository_watcher.rs:74` |
| 3 | Pasar el ratón por una lista re-renderiza toda la ventana, clonando en profundidad el estado del repositorio | 0,66 ms–2,38 ms × 20-60 veces/s | `src/ui/main_window.rs:1984` |

Fuera de esos tres, hay dos bloqueos del hilo de UI que se notan como tirones (§4.6, §4.7).

> **Antes de tocar nada, verifica esto:** `target/debug/git-helper.exe` tiene una fecha de
> modificación **posterior** a `target/release/git-helper.exe` (09:52 vs 09:34), y el `README.md:90`
> indica `cargo run`, que compila en modo debug. Si las pruebas se han hecho sobre la build de
> debug, parte de la lentitud percibida es eso. Confirma con `cargo run --release` antes de invertir
> esfuerzo. Dicho esto, §5 explica por qué el perfil de compilación **no** es la causa principal:
> lo medí y da mucho menos de lo que parecía.

---

## 2. Cómo se midió

- Máquina: Windows 11 Pro 26200, repositorio de pruebas = este mismo (`target/` con
  **10 763 ficheros**, `.git` completo).
- Los tiempos de `git` se tomaron con binarios Rust compilados con `rustc -O` y con el propio
  `GitClient` de la aplicación, no desde la shell, para no medir el arranque de bash.
- Cada cifra es la media de 3–40 repeticiones, con calentamiento previo de la caché de disco.
- El código de los benchmarks está en el §7 para que se pueda reproducir. **Los ficheros
  temporales que creé para medir ya han sido borrados; el árbol de trabajo está limpio.**

**Aviso importante sobre esta máquina:** crear un proceso aquí cuesta ~150 ms
(`git --version`, que no hace absolutamente nada, tarda 120–175 ms). En un equipo sin ese lastre
(antivirus en tiempo real, sandbox) serían 15–40 ms. Esto **no invalida** los hallazgos, pero cambia
las proporciones: en esta máquina "número de procesos lanzados" domina sobre todo lo demás; en un
equipo normal los hallazgos de UI (§4.3–§4.7) pesan relativamente más. **Optimizar el número de
procesos es correcto en ambos casos.**

---

## 3. El coste real, desglosado

Componentes de un `snapshot()`, medidos individualmente a través del `GitClient` de la aplicación:

```
git --version   (no hace nada)   120,7 · 133,1 · 149,9 · 159,9 · 174,3 ms   <- coste puro de lanzar un proceso
status                           217,3 ms   /   144,6 ms
remotes                          274,5 ms   /   172,7 ms
has_head                         163,1 ms   /   125,9 ms
history(201)                     457,8 ms   /   427,6 ms   <- el más caro, y la vista activa no lo usa
                                 ------------------------
snapshot() completo              681,8 · 562,9 · 679,1 ms   (2 077 ms con caché fría)
```

Procesos `git.exe` por acción del usuario (contados sobre el código):

| Acción | Procesos | Desglose |
|---|---|---|
| Arranque, 1 pestaña | 1 + 4 | `detect_version` (bloquea el resto) → `snapshot` |
| Arranque, 3 pestañas | 1 + 12 | `detect_version` y luego 3 × `snapshot` (concurrentes entre repos, en serie dentro de cada uno) |
| Clic en "Stage" | 4 + 4 (+4) | `add` → `snapshot` → **el watcher dispara otro `snapshot`** |
| Clic en "Unstage" | 5 + 4 (+4) | `has_head` + `restore` → `snapshot` → watcher |
| Cambiar de pestaña interna | 0 | pero hace un `fsync` (§4.6) |

`run_mutation` (`src/ui/main_window.rs:1188`) llama a `refresh_repository` al terminar **cualquier**
mutación, y `refresh_repository` siempre hace el `snapshot()` completo, historial incluido.

---

## 4. Hallazgos confirmados

### 4.1 `snapshot()` serializa 4-5 procesos independientes y siempre carga el historial
**CRÍTICO** · `src/git/client.rs:117-133`

```rust
let status = self.status(repository_root, cancellation)?;          // 118
let remotes = self.remotes(repository_root, cancellation)?;        // 119
let upstream = resolve_upstream(&status, &remotes);
let commits = self.history(repository_root, history_limit + 1, 0, cancellation)?;   // 121
```

Tres problemas apilados:

1. **Son secuenciales aunque sean independientes.** `status`, `remote` y `log` no se pisan entre
   sí. Medido: 4 comandos en serie = **791,0 ms**; los mismos 4 concurrentes = **476,6 ms**
   (**1,66×**). El techo es ~2× porque `log` domina.
2. **`history()` llama a `has_head()` primero** (`client.rs:198`), o sea un proceso `git rev-parse`
   extra en cada refresco solo para comprobar algo que casi nunca cambia.
3. **El historial se carga siempre, aunque la vista activa sea "Cambios"** (que es la vista por
   defecto, `RepositoryView::default() == Changes`). Es el comando más caro de todos: **427–475 ms**
   de los ~600 ms del snapshot, tirados a la basura hasta que el usuario pulse la pestaña
   "Historial".

**Arreglo.** (a) Ejecutar `status`, `remote` y `log` concurrentemente (el `ProcessRunner` ya es
`Send + Sync`, y `GitClient` es `Clone`; basta con `background_spawn` de los tres y un `join`).
(b) Sacar el historial de `snapshot()` y cargarlo de forma diferida cuando se selecciona la pestaña
Historial, cacheándolo por repositorio. (c) Cachear la lista de remotes e invalidarla solo cuando
cambie `.git/config`. (d) Eliminar el `has_head` previo: dejar que `git log` falle y tratar el error
"does not have any commits yet" (o consultar `HeadState::Unborn`, que `status` **ya devuelve** —
`src/domain/status.rs:70-79`, así que la información ya está disponible sin lanzar otro proceso).

**Ganancia estimada:** refresco de ~600 ms a ~150-200 ms (el `status`, que sí es imprescindible).
En vista Cambios, de 4-5 procesos a 1.

**Riesgo:** ninguno para la corrección de datos si se mantiene `resolve_upstream` recibiendo ambos
resultados ya resueltos. Ojo con no romper `GenerationGate` / `refresh_generation`
(`main_window.rs:415`): al paralelizar, el snapshot debe seguir descartándose completo si la
generación ha avanzado, no mezclar mitades de dos refrescos distintos.

---

### 4.2 El watcher se autoalimenta y vigila 10 763 ficheros de `target/`
**CRÍTICO** · `src/watcher/repository_watcher.rs:74` y `src/ui/main_window.rs:445-510`

```rust
watcher.watch(repository_root, RecursiveMode::Recursive)   // :74  -- incluye .git/ y target/
```

`git_directory` no se vigila por separado porque `starts_with(repository_root)` es cierto
(`:79`) — es decir, **`.git/` queda dentro de la vigilancia recursiva.**

**El bucle de realimentación está confirmado empíricamente:**

```
$ touch src/lib.rs ; stat .git/index ; git status --porcelain=v2 -z -uall ; stat .git/index
  plain status       : index REWRITTEN     <- el refresco se dispara a sí mismo
  --no-optional-locks: index NOT rewritten <- con este flag, no
```

El ciclo: el usuario guarda un fichero en su IDE → evento → debounce 250 ms → `refresh_repository`
→ `git status` **reescribe `.git/index`** → el watcher ve la escritura en `.git/index` → debounce
250 ms → **otro refresco completo de ~600 ms**. Cada pulsación de "guardar" en el editor cuesta como
mínimo el doble de lo que debería, y con `target/` vigilado (10 763 ficheros) cualquier `cargo build`
genera una tormenta continua de eventos.

Además no se filtra nada de ruido: `.git/objects/**`, `.git/logs/**`, `index.lock`, `FETCH_HEAD`,
`COMMIT_EDITMSG`, `ORIG_HEAD`, ni directorios ignorados por git (`target/`, `node_modules/`,
`dist/`). En Windows `notify` usa `ReadDirectoryChangesW`, cuyo búfer se desborda con árboles de
este tamaño y entonces empieza a **descartar** eventos y a reportar errores, que el worker trata
como si fueran cambios reales (`:99-101`), añadiendo refrescos falsos.

**Arreglo, en tres capas (aplicar las tres):**
1. **Cortar el bucle en el origen:** añadir `GIT_OPTIONAL_LOCKS=0` al entorno de los comandos de
   *solo lectura* en `src/git/client.rs:638`, junto al `GIT_TERMINAL_PROMPT` que ya está. Verificado:
   evita la reescritura del índice. **No aplicarlo a las mutaciones** (`add`, `restore`, `commit`),
   que sí necesitan escribir. Contrapartida: sin refrescar la caché de `stat`, algún `status`
   posterior puede ser algo más lento; medido aquí como irrelevante (307 ms vs 346 ms, dentro del
   ruido).
2. **Filtrar eventos por ruta** antes de reiniciar el debounce: descartar todo lo que esté bajo
   `.git/` salvo una lista blanca (`HEAD`, `refs/**`, `packed-refs`, `MERGE_HEAD`, `REBASE_HEAD`,
   `CHERRY_PICK_HEAD`) y descartar rutas ignoradas por git.
3. **No re-renderizar si nada cambió:** `RepositorySnapshot` ya deriva `PartialEq`
   (`src/domain/repository.rs:71`). En `finish_refresh` (`main_window.rs:517`), comparar con el
   snapshot anterior y **omitir `cx.notify()`** si son iguales. Esto amortigua cualquier refresco
   espurio que se cuele, sea de esta causa o de otra.

**Riesgo:** filtrar de más haría que la UI no reaccionase a cambios reales. La lista blanca de
`.git` debe incluir `index` (es lo que indica que el área de staging cambió) — por eso la capa 1
(`GIT_OPTIONAL_LOCKS=0`) es la que resuelve el conflicto: con ella, un cambio en `.git/index` solo
ocurre cuando algo lo ha modificado de verdad.

---

### 4.3 Pasar el ratón por una lista re-renderiza toda la ventana y clona el estado en profundidad
**ALTO** · `src/ui/main_window.rs:1984`, `:1815`, `:1425`

Esto es lo que hace que desplazarse por las listas se sienta pastoso, y es un mecanismo de GPUI que
conviene entender bien porque no es obvio.

Leído en el código de GPUI de este mismo `rev`
(`crates/gpui/src/elements/div.rs:2796-2806`):

```rust
window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
    let hovered = hitbox.is_hovered(window);
    let was_hovered = hover_state.as_ref().is_some_and(|state| state.borrow().element);
    if phase == DispatchPhase::Capture && hovered != was_hovered {
        if let Some(hover_state) = &hover_state {
            hover_state.borrow_mut().element = hovered;
            cx.notify(current_view);      // <-- re-render de la vista COMPLETA
        }
    }
});
```

Todas las filas de ambas listas llevan `.hover(...)` (`main_window.rs:1632` para las filas de cambios, `:1846` para las de historial), y también
cada `action_button` (`:2176`) y cada pestaña (`:1251`, `:1269`, `:1286`, `:2201`). `current_view` es la vista que construyó el elemento, que aquí es
**`MainWindow`**. Resultado: **cada vez que el ratón cruza el borde de una fila se ejecuta
`MainWindow::render()` completo** (dos veces en realidad: una por la fila que se abandona y otra por
la que se entra). A velocidad normal de ratón sobre filas de 40 px son ~20-60 renders por segundo.

Y cada uno de esos renders hace tres clones profundos del estado:

```rust
let active_repository = self.active_repository().cloned();          // :1984  RepositorySession entera
let commits = Arc::new(repository.snapshot.commits.clone());        // :1815  los 200 commits OTRA VEZ
let rows = Arc::new(build_change_rows(...));                        // :1425  4 pasadas + clon de cada FileChange
```

Coste medido (build de debug, que es la relevante si se usa `cargo run`):

| Repo | `.cloned()` | `commits.clone()` | `build_change_rows` | **total por render** |
|---|---|---|---|---|
| 300 cambios / 200 commits | 0,299 ms | 0,113 ms | 0,245 ms | **0,657 ms** |
| 2 000 cambios / 200 commits | 0,684 ms | 0,119 ms | 1,577 ms | **2,380 ms** |

En release baja a 0,49 ms y 1,90 ms respectivamente. A 60 renders/s con 2 000 cambios son ~143 ms/s
de puro clonado, más el coste de reconstruir el árbol de elementos y el layout.

Nótese que `build_change_rows` (`:2030`) recorre `changes` **cuatro veces** y clona cada `FileChange`
en cada pasada, y se rehace íntegro en cada frame aunque `collapsed_groups` y el snapshot no hayan
cambiado. Eso también anula parte del beneficio de `uniform_list`: la lista virtualiza el *render* de
las filas, pero el `Vec` completo de filas se reconstruye igual.

**Arreglo.**
1. No clonar la sesión en `render`: reestructurar para tomar prestado (`&RepositorySession`) o, más
   simple y con menos fricción con el borrow checker, envolver el snapshot en
   `Arc<RepositorySnapshot>` dentro de `RepositorySession`, de modo que `.cloned()` sea un
   incremento de contador. Esto arregla los tres clones de golpe, porque `render_history` pasaría a
   clonar el `Arc`, no el `Vec`.
2. Cachear el `Vec<ChangeListRow>` (o mejor, un `Arc<Vec<...>>`) e invalidarlo solo cuando cambien
   el snapshot o `collapsed_groups`.
3. Opcional pero es el arreglo de fondo: extraer la lista de cambios y la de historial a sus propias
   entidades GPUI (`cx.new(...)`). Así el `cx.notify(current_view)` del hover solo re-renderiza esa
   lista y no la ventana entera. Es lo que hace Zed y es la razón por la que su UI aguanta listas
   enormes.

**Riesgo:** si se cachean las filas, la invalidación tiene que cubrir *todas* las entradas —
snapshot, `collapsed_groups` y el `repository_id` activo — o la UI mostrará datos rancios. Un
`PartialEq` sobre el snapshot (ya derivado) es suficiente como llave de invalidación.

---

### 4.4 Espera del proceso hijo por sondeo fijo de 25 ms
**MEDIO** (aquí; **ALTO** en un equipo normal) · `src/process.rs:151-176`

```rust
let status = loop {
    ...
    if let Some(status) = child.try_wait().map_err(ProcessError::Wait)? { break status; }
    thread::sleep(Duration::from_millis(25));       // :175
};
```

Todo comando git paga hasta 25 ms de latencia añadida (media ~12,5 ms) después de haber terminado
ya. Con 4-10 procesos por acción son 50-250 ms de espera pura.

**Honestidad sobre la magnitud:** lo medí con 40 iteraciones alternadas y en **esta** máquina queda
dentro del ruido, porque los ~150 ms de creación de proceso lo tapan:

```
current: fixed 25ms poll           152,7 ms/iter
candidate: adaptive backoff        163,9 ms/iter
candidate: waiter thread           154,9 ms/iter
```

Pero en un equipo donde `git --version` tarda 15 ms, un sondeo de 25 ms **casi triplica** el coste
de cada comando corto. Merece arreglarse; no es la causa principal.

**Arreglo recomendado: backoff adaptativo** (empezar en ~150 µs y duplicar hasta un techo de 25 ms).
Es tres líneas, no cambia la semántica de cancelación ni de timeout, y no tiene riesgo.

Descarté dos alternativas que parecen mejores y no lo son:
- *Hilo dedicado que hace `child.wait()` y avisa por canal*: es el ideal teórico (latencia cero,
  cero sondeo del hijo), pero `kill()` necesita `&mut Child`, así que habría que meter el `Child` en
  un `Arc<Mutex<...>>` y el hilo que espera retendría el mutex, bloqueando precisamente la
  cancelación que se quiere conservar.
- *Usar el EOF de las pipes como señal de salida*: rompería con un proceso nieto que herede los
  descriptores (típicamente un credential helper de git en un `push`/`fetch`), colgando la espera
  hasta el timeout de 15 minutos.

Nota secundaria: cada invocación crea 2-3 hilos de SO (`:137-141`). Con decenas de comandos por
minuto no es dramático, pero un pool o lecturas no bloqueantes lo evitarían.

*(Comprobado y descartado: resolver `git` por PATH no cuesta nada aquí — ver §5.)*

---

### 4.5 Cerrar una pestaña bloquea el hilo de UI hasta 1 segundo
**ALTO** (muy visible aunque sea puntual) · `src/watcher/repository_watcher.rs:121-129`

```rust
impl Drop for RepositoryWatcher {
    fn drop(&mut self) {
        let _ = self.stop_sender.send(());
        if let Some(worker) = self.worker.take() && worker.join().is_err() { ... }   // :125
    }
}
```

El worker solo consulta `stop_receiver` al principio de cada iteración (`:91`), y cuando está
inactivo se queda bloqueado en `event_receiver.recv_timeout(Duration::from_secs(1))` (`:94`). Así
que `join()` espera **hasta 1 s** (media ~0,5 s) a que expire ese timeout.

Y ese `drop` ocurre en el hilo de UI: `close_repository` (`main_window.rs:336`) hace
`self.repository_watchers.remove(&repository_id)`, dentro de un `cx.listener`. La ventana se
congela.

**Arreglo.** Hacer que el worker despierte inmediatamente al recibir la señal de parada: usar un
único canal para eventos y parada (un `enum { Event(..), Stop }`), o `crossbeam::select!`, o
simplemente dejar de esperar el `join()` (soltar el handle y que el hilo termine solo, ya que no
comparte estado que haya que ordenar). Lo más limpio: canal unificado.

---

### 4.6 `fsync` en el hilo de UI al cambiar de pestaña
**MEDIO** · `src/persistence/app_state_store.rs:242` + 6 llamadas en `main_window.rs`

`AppStateStore::save` hace `sync_all()` (`:242`), que es un `fsync` — en Windows entre 5 y 50 ms, más
en discos ocupados o cifrados con BitLocker. Se llama sincrónicamente desde el hilo de UI en
`main_window.rs:289, 304, 351, 387, 563, 1216`. Dos de esas están en el camino interactivo puro:

- `:563` — `select_view`: **cada clic en las pestañas "Cambios"/"Historial"**.
- `:1216` — `select_repository`: **cada clic para cambiar de repositorio.**

Cambiar de vista no debería tocar el disco de forma sincrónica.

**Arreglo.** Mover el guardado a `background_spawn`, con "debounce"/coalescencia (guardar como
mucho una vez por segundo, o al perder el foco y al cerrar). El estado persistido es pequeño y no
crítico: perder el último cambio de pestaña ante un fallo no tiene coste. Alternativa mínima: quitar
el `sync_all()` — el `NamedTempFile` + `persist()` ya da atomicidad; el `fsync` solo protege contra
corte de corriente, algo desproporcionado para "qué pestaña estaba mirando".

---

### 4.7 Trabajo sincrónico de disco antes del primer pixel
**MEDIO** · `src/ui/main_window.rs:105-114`

`MainWindow::new` corre en el hilo de UI antes de que la ventana pueda pintar:

```rust
let mut state = state_store.as_ref().and_then(|store| store.load().ok())...   // :107  lee y parsea JSON
state.repositories.retain(|repository| repository.root_path.is_dir());        // :112
```

El `retain` con `is_dir()` toca **cada** repositorio restaurado. Si alguno está en una unidad de red
desconectada o en un disco externo dormido, `is_dir()` se bloquea en el timeout de SMB o en el
arranque del disco: **segundos** de ventana en blanco.

Y en `initialize` (`:151`) todos los refrescos quedan detrás de `detect_version` (`:173`), que es un
proceso git más (~150 ms aquí) antes de que empiece el primer `snapshot`. La versión de git solo se
usa para la barra de estado y para poner un mensaje de error: no debería bloquear la carga de datos.

**Arreglo.** Mover `load()` y la validación `is_dir()` a `background_spawn`, pintando la ventana con
las pestañas en estado "cargando". Lanzar `detect_version` **en paralelo** con los primeros
`snapshot()`, no antes.

---

### 4.8 `commit_input_subscriptions` crece sin límite
**BAJO** · `src/ui/main_window.rs:87, 230` vs `:320-352`

`create_commit_input` hace `push` de una `Subscription` (`:230`) y `close_repository` **no la
elimina** (`:320-352` limpia `commit_inputs`, `selected_commit_details`, `repository_watchers` y
`collapsed_groups`, pero no este `Vec`). Abrir y cerrar pestañas durante una sesión larga acumula
suscripciones muertas.

El impacto real es pequeño (la entidad observada ya no existe, así que no se disparan), pero es una
fuga y es trivial de corregir: guardar las suscripciones en un
`HashMap<RepositoryId, Subscription>` y eliminarlas en `close_repository`.

---

### 4.9 Cada tecla en el mensaje de commit re-renderiza toda la ventana
**BAJO tras arreglar §4.3** · `src/ui/commit_input.rs:124-127` + `src/ui/main_window.rs:225-228`

`CommitInput::changed` emite `CommitMessageChanged` en cada pulsación, y `MainWindow` está suscrito
con un `cx.notify()` (`main_window.rs:226-228`). O sea: **una tecla = un `MainWindow::render()`
completo**, con sus tres clones profundos del §4.3.

Lo listo como BAJO porque la suscripción es *necesaria* (el contador de staged y el `enabled` del
botón Commit dependen del contenido) y porque, una vez arreglado §4.3, un render pasa a ser barato.
No hace falta tocar esto si se arregla §4.3; sí conviene revisarlo si no se arregla.

---

## 5. Hipótesis que medí y **descarté** (no perder tiempo aquí)

Las anoto explícitamente porque son las sospechas naturales y todas parecen ciertas hasta que se
miden:

| Hipótesis | Veredicto | Evidencia |
|---|---|---|
| **El perfil de compilación es la causa principal**: no hay `[profile.dev]` y Zed sí pone `taffy = { opt-level = 3 }` | **Mayormente descartada** | Compilé un banco de pruebas de taffy 0.13 (el mismo del `Cargo.lock`) con 401 nodos, equivalente a ~40 filas visibles: **0,452 ms/frame a opt-level 0 vs 0,364 ms a opt-level 3**. Solo **1,24×**, y 0,09 ms por frame en absoluto: irrelevante frente a los 600 ms del §4.1. *Sigue mereciendo la pena añadir `[profile.dev]` (es gratis y ayuda algo con el shaping de texto, que no pude aislar), pero no como prioridad.* |
| **Resolver `git` por PATH en cada spawn es caro** (`GitClient::default()` usa `PathBuf::from("git")`) | **Descartada** | Medido: `"git"` = 153,5 ms/iter vs ruta absoluta = 150,3 ms/iter. Sin diferencia. *(Podría importar en una máquina con un PATH gigantesco o con entradas en unidades de red; aquí no.)* |
| **El sondeo de 25 ms es el cuello de botella** | **Descartada como causa principal** | Está acotado a 25 ms; los ~150 ms de creación de proceso dominan. Ver §4.4: real pero secundario. |
| **`--untracked-files=all` es caro con `target/` de 10 763 ficheros** | **Descartada** | `-uall` = 307 ms vs `-unormal` = 336 ms. Sin diferencia (`target/` está *ignorado*, no sin seguimiento, y git poda el directorio entero). **No cambiar este flag**: `all` es necesario para listar archivos individuales en la UI. |
| **`uniform_list` no virtualiza** | **Descartada** | Virtualiza correctamente: el closure de `cx.processor` solo recibe el rango visible (`main_window.rs:1451`, `:1827`). El problema no es la virtualización sino el `Vec` de filas que se construye completo antes (§4.3). |
| Parsers `status_parser.rs` / `log_parser.rs` con coste cuadrático | **No confirmada** | No encontré patrones cuadráticos. No es donde está el tiempo: el `status` completo (proceso + parseo) son 145-217 ms y el proceso solo ya son ~150. Baja prioridad. |

---

## 6. Orden de aplicación recomendado

Ordenado por ganancia percibida por euro de esfuerzo. **Medir después de cada paso**, porque los
tres primeros se solapan: al arreglar §4.2, la mitad de los refrescos desaparecen, así que el
beneficio absoluto de §4.1 se reduce (y viceversa).

1. **§4.1(b) — Sacar el historial de `snapshot()`.** El cambio con mejor relación
   ganancia/esfuerzo de toda la lista: elimina 427-475 ms del refresco por defecto y es
   conceptualmente correcto (no cargues lo que no se está mirando).
2. **§4.2 — Cortar el bucle del watcher** (las tres capas: `GIT_OPTIONAL_LOCKS=0`, filtrado de
   rutas, y omitir `notify()` si el snapshot no cambió). Divide por dos el número de refrescos y
   elimina las tormentas durante `cargo build`.
3. **§4.1(a,c,d) — Paralelizar `status`/`remote` y eliminar `has_head` y `remotes` redundantes.**
   1,66× medido en lo que quede del snapshot.
4. **§4.3 — `Arc<RepositorySnapshot>` + caché de `Vec<ChangeListRow>`.** Es lo que arregla la
   sensación pastosa al mover el ratón. Hacer los puntos 1 y 2 (`Arc` y caché); el 3 (entidades
   GPUI separadas) es refactor mayor, dejarlo para después y solo si sigue notándose.
5. **§4.5 — Canal unificado en el worker del watcher.** Quita un congelado de hasta 1 s al cerrar
   pestaña. Arreglo pequeño y muy visible.
6. **§4.6 — Guardado de estado asíncrono y con coalescencia.**
7. **§4.7 — Arranque: `load()`/`is_dir()` al background, `detect_version` en paralelo.**
8. **§4.4 — Backoff adaptativo en `process.rs`.** Poca ganancia *en esta máquina*, notable en
   equipos rápidos. Tres líneas, riesgo nulo.
9. **§4.8 — Fuga de suscripciones.** Corrección de higiene.
10. **Añadir `[profile.dev]`** (siguiendo a Zed: `codegen-units = 16`, `debug = "limited"`, y
    `opt-level = 3` para `taffy`, `rustybuzz`, `ttf-parser`, `swash`, `cosmic-text` — todos están en
    el `Cargo.lock`). Y en release, añadir `codegen-units = 1` a lo que ya hay (`lto = "thin"`), que
    es lo que usa Zed. Ganancia modesta y medida (§5), pero es coste cero.

**§4.9 no requiere acción propia** si se hace el punto 4.

### Escenario combinado, antes y después

Usuario editando código en su IDE, con un repositorio de 300 archivos modificados abierto en
Git Helper. Guarda un fichero:

- **Hoy:** evento → 250 ms debounce → snapshot (≈600 ms, 4 procesos) → `git status` reescribe
  `.git/index` → 250 ms debounce → **segundo snapshot completo (≈600 ms)** → 2 renders con clonado
  profundo. **≈1,7 s de trabajo y 8 procesos `git.exe` por cada Ctrl+S.**
- **Tras los pasos 1-4:** evento → 250 ms debounce → un `git status` (≈150 ms, 1 proceso) → sin
  reescritura del índice, sin segundo refresco → si el snapshot no cambió, ni un render.
  **≈150 ms y 1 proceso.** Del orden de **10× menos trabajo**.

---

## 7. Reproducir las mediciones

Los ficheros temporales que usé ya están borrados. Para rehacerlas, `examples/bench.rs` en el
proyecto (requiere añadir `examples/` temporalmente):

```rust
use std::{path::PathBuf, time::Instant};
use git_helper::{git::GitClient, process::CancellationToken};

fn t<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let value = f();
    println!("  {label:<26} {:>7.1} ms", start.elapsed().as_secs_f64() * 1000.0);
    value
}

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".to_owned()))
        .canonicalize().unwrap();
    let client = GitClient::default();
    let token = CancellationToken::default();
    let _ = client.detect_version(&token);
    println!("-- coste puro de lanzar un proceso --");
    for _ in 0..5 { t("git --version", || client.detect_version(&token).unwrap()); }
    println!("-- componentes del snapshot --");
    for _ in 0..2 {
        t("status", || client.status(&root, &token).unwrap());
        t("remotes", || client.remotes(&root, &token).unwrap());
        t("has_head", || client.has_head(&root, &token).unwrap());
        t("history(201)", || client.history(&root, 201, 0, &token).unwrap());
    }
    println!("-- snapshot completo = coste de un refresco --");
    for _ in 0..3 { t("snapshot", || client.snapshot(&root, 200, &token).unwrap()); }
}
```

```sh
cargo build --release --example bench && ./target/release/examples/bench.exe .
```

Y la comprobación del bucle del watcher, que es la más importante y la más rápida:

```sh
touch src/lib.rs; stat -c '%y' .git/index
git status --porcelain=v2 -z --untracked-files=all >/dev/null
stat -c '%y' .git/index          # cambia  -> el refresco se dispara a sí mismo

touch src/lib.rs; stat -c '%y' .git/index
git --no-optional-locks status --porcelain=v2 -z --untracked-files=all >/dev/null
stat -c '%y' .git/index          # no cambia -> bucle roto
```
