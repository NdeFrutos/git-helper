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

## Pestañas y navegación

- [ ] Se pueden abrir varios repositorios en pestañas distintas.
- [ ] Con diez pestañas abiertas, la barra conserva un control de overflow horizontal y cada pestaña
  sigue identificándose por nombre truncado y ruta completa accesible.
- [ ] Con dos o más repositorios abiertos, la pestaña `Resumen (n)` muestra una vista compacta de
  todos ellos sin lanzar fetch ni lecturas Git adicionales al renderizar.
- [ ] Cada fila del resumen muestra repositorio, rama, contadores, sync, estado y frescura remota;
  al pulsarla o usar `Enter` se abre la sesión correspondiente.
- [ ] `Ctrl+0` abre el resumen; con el foco en la vista, `↑`/`↓`/`Enter` navegan las filas.
- [ ] `Ctrl+Tab` / `Ctrl+Shift+Tab` cambian de pestaña.
- [ ] El botón de cerrar en cada pestaña cierra solo esa sesión.
- [ ] Las pestañas internas `Changes` e `History` cambian sin perder el estado de la otra.

## Vista Changes

- [ ] Los grupos (staged, unstaged, conflictos) muestran contador y se pueden colapsar o expandir.
- [ ] Cada fila muestra código de estado, nombre de archivo y ruta padre truncada si aplica.
- [ ] Al estrechar la ventana, el nombre y la ruta se truncan con elipsis y las acciones de la fila
  siguen en la misma línea: las filas tienen altura fija, así que nada debe desbordar hacia la fila
  siguiente.
- [ ] `Stage` / `Unstage` por archivo y `Stage todo` / `Unstage todo` actualizan la lista.
- [ ] `Descartar` muestra confirmación antes de ejecutar.
- [ ] La barra de rama y los botones Fetch / Pull / Push muestran estados de carga y errores legibles.
- [ ] Una ruta larga se trunca en la barra inferior sin ocultar el estado ni la versión de Git.

## Commit y Cursor CLI

- [ ] El cuadro de mensaje admite varias líneas, pegado y atajos básicos.
- [ ] `Commit` falla con mensaje claro si no hay cambios staged o el mensaje está vacío.
- [ ] `Generar mensaje` (o atajo configurado) funciona cuando Cursor CLI está disponible.
- [ ] Si hay patrones sensibles en el diff staged, se solicita consentimiento explícito.

## Vista History

- [ ] La lista de commits es scrollable y virtualizada (sin tirones con muchos commits).
- [ ] Al seleccionar un commit se muestran autor, fecha, asunto y cuerpo si existe.
- [ ] `Cargar más` añade entradas sin duplicar las ya visibles.

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
