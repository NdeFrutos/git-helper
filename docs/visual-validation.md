# Validación visual manual

Checklist para comprobar la interfaz de Git Helper en Windows antes de distribuir un binario o MSI.
La comprobación compacta de UX-06 usa una ventana de 960×640 y se repite con escala DPI del
100 %, 125 %, 150 % y 200 % cuando el entorno lo permite.

## Preparación

1. Compilar en release: `cargo build --release`.
2. Tener al menos un repositorio Git local con cambios staged, unstaged y sin cambios.
3. Tener Git for Windows instalado y accesible en `PATH`.
4. Opcional: tener Cursor CLI (`agent`) autenticado para probar la generación de mensajes.

Para UX-06, guardar una captura por tamaño/DPI en la evidencia de la revisión. La captura debe
incluir la barra de pestañas, la rama, la lista de cambios y la zona de commit; no se deben incluir
credenciales ni contenido sensible.

## Arranque y persistencia

- [ ] La ventana abre sin bloquearse y muestra el estado vacío si no hay repositorios guardados.
- [ ] Al abrir un repositorio, la pestaña superior muestra el nombre del directorio y el tooltip o
  etiqueta accesible incluye la ruta completa.
- [ ] Al cerrar y volver a abrir la aplicación, se restauran las pestañas, la pestaña activa, la
  geometría de ventana y el borrador del mensaje de commit.
- [ ] Un commit fallido conserva el borrador; un commit exitoso lo limpia.
- [ ] Si un repositorio guardado no está accesible (disco desconectado), la pestaña permanece con
  aviso, botón Reintentar y Cerrar; al recuperar la ruta, el estado vuelve a cargarse.
- [ ] El estado vacío muestra repositorios recientes persistidos.
- [ ] Tras reiniciar se conservan el orden de las pestañas y los favoritos fijados.

## Pestañas y navegación

- [ ] Se pueden abrir varios repositorios en pestañas distintas.
- [ ] Con diez pestañas abiertas, la barra conserva un control de overflow horizontal y cada pestaña
  sigue identificándose por nombre truncado y ruta completa accesible.
- [ ] `Ctrl+Tab` / `Ctrl+Shift+Tab` cambian de pestaña.
- [ ] El botón de cerrar en cada pestaña cierra solo esa sesión.
- [ ] Las pestañas internas `Changes` e `History` cambian sin perder el estado de la otra.

## Orden, favoritos y reapertura (UX-10)

- [ ] Arrastrar una pestaña sobre otra la coloca en esa posición; durante el arrastre se ve una
  vista previa con el nombre y la pestaña de destino se resalta.
- [ ] `Ctrl+Shift+PageUp` / `Ctrl+Shift+PageDown` mueven la pestaña activa; en el primer y último
  hueco no dan la vuelta y la barra de estado lo indica.
- [ ] Tras mover una pestaña, su borrador de commit, su vista seleccionada y cualquier operación en
  curso siguen siendo los mismos; el repositorio activo no cambia.
- [ ] Con veinte pestañas abiertas, arrastrar y reordenar con teclado sigue siendo fluido y la barra
  mantiene el scroll horizontal.
- [ ] La estrella de cada pestaña fija y desfija el repositorio (`Ctrl+Shift+B`): muestra `★` cuando
  es favorito y `☆` cuando no.
- [ ] El estado vacío y el panel de clonado listan `Favoritos`; una entrada ya abierta se muestra
  como `— abierto`, distinta de una pestaña normal.
- [ ] Abrir un favorito que ya está abierto activa su pestaña en lugar de duplicarla, también si la
  ruta se escribió con otra capitalización o con `/`.
- [ ] Con un favorito en una unidad desconectada: la fila pasa a `— no disponible`, se puede
  reintentar y `✕` lo quita de favoritos sin borrar nada del disco (comprobar la carpeta después).
- [ ] Cerrar una pestaña permite recuperarla: el botón `↩` de la barra y `Ctrl+Shift+T` la reabren
  sin crear una segunda pestaña del mismo repositorio.
- [ ] Repetir el bloque anterior con escala DPI del 100 %, 125 %, 150 % y 200 %: la estrella, `✕` y
  `↩` siguen siendo pulsables y no desbordan la barra.

## Vista Changes

- [ ] Los grupos (staged, unstaged, conflictos) muestran contador y se pueden colapsar o expandir.
- [ ] Cada fila muestra código de estado, nombre de archivo y ruta padre truncada si aplica.
- [ ] Al estrechar la ventana, el nombre y la ruta se truncan con elipsis y las acciones de la fila
  siguen en la misma línea: las filas tienen altura fija, así que nada debe desbordar hacia la fila
  siguiente.
- [ ] `Stage` / `Unstage` por archivo y `Stage todo` / `Unstage todo` actualizan la lista.
- [ ] Con la ventana estrecha, las cuatro acciones de fila (`Abrir`, `Mostrar`, `Descartar`,
  `Stage`) permanecen en su fila de 56 px y lo que se trunca es el nombre y la ruta.
- [ ] `Ctrl+F` enfoca el filtro; al escribir, la cabecera muestra la consulta y `n de m archivos`.
- [ ] Con filtro activo, un archivo modificado en índice y worktree sigue apareciendo en ambos
  grupos y cada fila hace stage/unstage solo de su estado.
- [ ] Con filtro activo desaparecen `Stage todo` y `Unstage todo`.
- [ ] Una consulta sin coincidencias muestra el estado vacío, no una lista en blanco.
- [ ] `Escape` dentro del filtro y el botón `Limpiar` restauran la lista completa.
- [ ] `Descartar` muestra confirmación antes de ejecutar.

### Selección múltiple

- [ ] Un clic en una fila la resalta; `Ctrl+clic` añade y quita filas sueltas; `Mayús+clic`
  selecciona el rango visible entre el ancla y la fila pulsada; `Ctrl+Mayús+clic` suma el rango a
  lo ya seleccionado.
- [ ] La lista muestra un borde de acento cuando tiene el foco, y un clic en cualquier parte de
  ella (incluido el espacio vacío) se lo da.
- [ ] `Ctrl+Shift+S` / `Ctrl+Shift+U` no hacen nada mientras la vista `Historial` está abierta.
- [ ] Los botones `Stage selección (0)` / `Unstage selección (0)` atenuados no ejecutan nada al
  pulsarlos ni muestran un error.
- [ ] Con el foco en la lista, `↑` / `↓` mueven la marca de fila activa y `Mayús+↑` / `Mayús+↓`
  extienden el rango; la lista hace scroll para mantener visible la fila activa.
- [ ] `Ctrl+A` selecciona todas las filas visibles y `Esc` vacía la selección.
- [ ] El contador junto a los botones refleja el número de filas seleccionadas y los rótulos
  `Stage selección (n)` / `Unstage selección (n)` indican a cuántas rutas afectarán.
- [ ] Pulsar `Stage`, `Unstage` o `Descartar` de una fila ejecuta esa acción **sin** cambiar la
  selección.
- [ ] Un archivo staged y modificado de nuevo se selecciona por separado en cada grupo, y actuar
  sobre uno no altera el otro.
- [ ] Modificar archivos fuera de la aplicación quita de la selección las filas que desaparecen y
  no marca otras por su posición; plegar un grupo libera sus filas.
- [ ] Las filas de conflicto no se pueden seleccionar.
- [ ] Con una selección que incluya una ruta que Git rechace, el error indica cuántas rutas se
  aplicaron y el motivo de cada fallo, y la lista queda reconciliada con Git.
- [ ] La barra de rama y los botones Fetch / Pull / Push muestran estados de carga y errores legibles.
- [ ] Una ruta larga se trunca en la barra inferior sin ocultar el estado ni la versión de Git.

## Herramientas externas (UX-12)

- [ ] `Editor`, `Terminal` y `Explorador` de la barra de acciones abren el repositorio activo; el
  directorio de trabajo es su raíz aunque haya otra pestaña con el mismo nombre de carpeta.
- [ ] `Ctrl+Mayús+E`, `Ctrl+Alt+T` y `Ctrl+Mayús+X` hacen lo mismo desde el teclado.
- [ ] `Abrir` y `Mostrar` de una fila abren el archivo en el editor y lo seleccionan en el
  Explorador, incluso con espacios, acentos y `&` en la ruta.
- [ ] `Mostrar` sobre un archivo eliminado abre su carpeta y lo explica en la barra de estado, sin
  tratarlo como error.
- [ ] La terminal aparece visible y con su prompt utilizable (no se cierra al instante).
- [ ] Abrir cualquiera de las tres herramientas no congela la ventana ni interrumpe un refresco.
- [ ] Sin editor instalado —o con la ruta configurada movida— el aviso explica el problema y
  `Elegir editor…` permite seleccionar el ejecutable y reintentar la acción.
- [ ] Ninguna acción abre más de una aplicación ni ejecuta scripts del repositorio.

## Commit y Cursor CLI

- [ ] El cuadro de mensaje admite varias líneas, pegado y atajos básicos.
- [ ] `Commit` falla con mensaje claro si no hay cambios staged o el mensaje está vacío.
- [ ] `Generar mensaje` (o atajo configurado) funciona cuando Cursor CLI está disponible.
- [ ] Si hay patrones sensibles en el diff staged, se solicita consentimiento explícito.

## Vista History

- [ ] La lista de commits es scrollable y virtualizada (sin tirones con muchos commits).
- [ ] Al seleccionar un commit se muestran autor, fecha, asunto y cuerpo si existe.
- [ ] `Cargar más` añade entradas sin duplicar las ya visibles.
- [ ] `Ctrl+F` enfoca la búsqueda; la cabecera muestra consulta, resultados, commits explorados y si
  el recorrido sigue en marcha.
- [ ] Una consulta encuentra commits que todavía no estaban en la página cargada; `Buscar más`
  continúa el recorrido y `Cargar más` desaparece mientras hay búsqueda activa.
- [ ] Seleccionar un resultado carga sus detalles aunque no estuviera en el historial visible.
- [ ] Cambiar de rama con una búsqueda activa reinicia los resultados sobre la rama nueva y no
  mezcla commits de la anterior.
- [ ] Escribir deprisa no encadena procesos de Git ni bloquea la ventana; `Escape` limpia la consulta.
- [ ] Una búsqueda sin coincidencias en toda la referencia muestra el estado vacío.

## Estados de error y vacío

- [ ] Repositorio sin cambios muestra estado vacío comprensible.
- [ ] El árbol limpio, el repositorio sin commit inicial, la carga inicial, el fallo inicial y el
  estado anterior conservado tras un fallo de refresh se distinguen visualmente.
- [ ] El error muestra un resumen accionable; sus detalles técnicos se pueden expandir y copiar.
- [ ] Errores de Git (por ejemplo, pull no fast-forward) aparecen en la barra de estado sin colgar
  la UI.
- [ ] Operaciones largas muestran indicador de carga y se pueden cancelar si aplica.
- [ ] Al cancelar o agotar el tiempo una operación Git/Cursor, la sesión sale de `Running`, conserva
  el último snapshot visible y muestra un estado diferenciado; después se reconcilia el estado real.
- [ ] En Windows, repetir cancelaciones de una operación que lance descendientes no deja procesos
  ni abre ventanas de consola.

## Accesibilidad y DPI

- [ ] El texto es legible con escala del sistema al 100 %.
- [ ] Con escala 125 % o 150 %, los botones y filas no se solapan de forma inaceptable.
- [ ] Con escala 200 %, las acciones principales siguen visibles: la barra de rama y acciones puede
  repartirse en varias líneas (crece en alto), mientras que las filas de archivo mantienen su altura
  y truncan el nombre y la ruta en lugar de desbordarse.
- [ ] Los colores de estado no son el único indicador (también hay letras/códigos).

## Instalador (si aplica)

- [ ] El MSI instala `git-helper.exe`, `LICENSE` y `THIRD_PARTY_NOTICES.md`.
- [ ] El acceso directo y el icono en “Programas y características” usan `assets/icon.ico`.
- [ ] La desinstalación elimina los archivos instalados sin dejar el ejecutable en `bin`.

## Registro de UX-06

- [x] `cargo build --release --locked` completó correctamente en Windows durante esta revisión.
- [x] Inspección de la ventana compacta con capturas en `docs/evidence/ux-06/`:
  - `960x640-100.png`: tamaño por defecto (960×640) al 100 %. Pestañas, rama, lista de cambios y
    zona de commit visibles sin solapamientos.
  - `960x640-200.png`: el mismo estado con factor de escala 2 (equivalente a DPI 200 %). Las
    acciones principales siguen visibles y la jerarquía se mantiene.
  - `diez-pestanas-960x640.png`: diez repositorios abiertos en 960×640. La barra de pestañas
    conserva su scroll horizontal, el botón «+» queda fijo, los nombres se truncan y la ruta completa
    del repositorio activo sigue visible en la barra inferior. Con nombres de prefijo idéntico el
    texto truncado no basta para distinguirlos: la identificación depende de la ruta y del
    `aria_label` de cada pestaña.
  - `estrecha-300-antes.png` / `estrecha-300-despues.png`: ventana estrechada a 300 px. Antes del
    arreglo, las acciones de la fila saltaban de línea dentro de una fila de 56 px fijos y se
    solapaban con la fila siguiente; después, el nombre y la ruta se truncan y las acciones
    permanecen en su fila.
- [ ] Pendiente en Windows: repetir la inspección con el backend nativo y con la escala real del
  sistema al 125 % y 150 %.

## Registro de UX-12

- [x] `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`
  y `cargo test --all-targets --locked` en Linux. La única prueba roja es
  `git::path::tests::rejects_empty_absolute_and_parent_paths`, anterior a este cambio: comprueba que
  `C:\secreto.txt` es absoluta, algo que solo es cierto en Windows.
- [x] Comprobación funcional del lanzamiento real sobre un repositorio de prueba con rutas con
  espacios, acentos y `&`, usando un ejecutable de editor simulado que registra su `argv` y su
  directorio de trabajo:
  - La ruta llega como **un único argumento literal** junto a la opción configurada
    (`--new-window`), sin comillas añadidas ni troceo por espacios.
  - El directorio de trabajo es la raíz del repositorio en las dos variantes (archivo y raíz).
  - Un archivo eliminado devuelve `PathMissing` y un ejecutable movido devuelve `EditorMissing`,
    que es el error que ofrece `Elegir editor…`.
  - Sin terminal ni gestor de archivos instalados (contenedor sin escritorio), las acciones
    devuelven `TerminalNotFound` y `FileManagerNotFound` en vez de fallar en silencio.
- [x] `editor_command` sobrevive al ciclo de carga y guardado del estado persistido.
- [ ] Pendiente en Windows con build release: consola visible de la terminal (`wt.exe`,
  PowerShell o `cmd.exe`), selección del archivo con `explorer.exe /select,` y el diálogo
  `Elegir editor…`. No se pudieron comprobar aquí porque ninguno de esos programas existe en el
  entorno de revisión.
- [ ] Pendiente: inspección visual de la fila de cambios con sus cuatro acciones (`Abrir`,
  `Mostrar`, `Descartar`, `Stage`) en ventana estrecha y de la barra de acciones con los tres
  botones nuevos. El procedimiento de capturas de UX-06 no se pudo reproducir en esta revisión: con
  el backend X11 compilado, la aplicación arranca y conecta con Xvfb, pero no llega a mapear una
  ventana en este contenedor, así que no hay captura que comparar.

### Cómo se obtuvieron las capturas

Las capturas se tomaron en Linux, no en Windows, porque el entorno de revisión no dispone de una
sesión de escritorio Windows. Procedimiento, por si hay que reproducirlo:

1. `Xvfb :99 -screen 0 2600x1800x24` como servidor X sin gestor de ventanas.
2. Compilación local habilitando temporalmente la característica `x11` de `gpui`/`gpui_platform`
   (el `Cargo.toml` del repositorio no la activa porque el objetivo es Windows; sin ella GPUI arranca
   en modo headless y no crea ventana). Ese cambio **no** forma parte del commit.
3. `LOCALAPPDATA` apuntando a un directorio temporal con un `state.json` que abre un repositorio de
   prueba con cambios staged, sin stage, sin seguimiento y rutas largas.
4. `GPUI_X11_SCALE_FACTOR=2` para emular DPI 200 %; `xdotool windowsize` para estrechar la ventana e
   `import` para capturar.

Limitación conocida: el backend X11 de GPUI no es el de Windows, así que estas capturas validan el
layout (elipsis, altura de fila, overflow) pero no el renderizado ni la escala nativos de Windows.

## Registro de UX-08

Comprobado en Windows 11 durante esta revisión:

- [x] `cargo build --release --locked` completó correctamente (`target/release/git-helper.exe`).
- [x] `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`
  y `cargo test --all-targets --locked` en verde.
- [x] Cobertura automática del comportamiento buscable, sin interfaz: plegado Unicode y de
  separadores, consultas con caracteres especiales tratadas como literales, prefijo de hash solo
  para consultas hexadecimales, filtrado de 10.000 cambios, independencia de las filas staged y de
  worktree, desaparición de las acciones de grupo con filtro activo y descarte de páginas de
  historial procedentes de otra consulta, otra referencia u otro desplazamiento.

Pendiente de comprobación manual en una sesión de escritorio Windows; el entorno de esta revisión no
tiene una, así que la lista de `Vista Changes` y `Vista History` de arriba sigue sin marcar:

- [ ] Foco con `Ctrl+F`, limpieza con `Escape` y recorrido con tabulador dentro de la barra.
- [ ] Latencia percibida al teclear sobre un repositorio con 10.000 cambios y sobre un historial
  largo paginado.
- [ ] Renderizado de la barra de búsqueda con escala del sistema al 125 %, 150 % y 200 %.
