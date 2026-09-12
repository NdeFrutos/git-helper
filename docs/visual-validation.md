# Validación visual manual

Checklist para comprobar la interfaz de Git Helper en Windows antes de distribuir un binario o MSI.
Ejecutar en un monitor con escala DPI del 100 % y, opcionalmente, repetir al 125 % y 150 %.

## Preparación

1. Compilar en release: `cargo build --release`.
2. Tener al menos un repositorio Git local con cambios staged, unstaged y sin cambios.
3. Tener Git for Windows instalado y accesible en `PATH`.
4. Opcional: tener Cursor CLI (`agent`) autenticado para probar la generación de mensajes.

## Arranque y persistencia

- [ ] La ventana abre sin bloquearse y muestra el estado vacío si no hay repositorios guardados.
- [ ] Al abrir un repositorio, la pestaña superior muestra el nombre del directorio y el tooltip o
  etiqueta accesible incluye la ruta completa.
- [ ] Al cerrar y volver a abrir la aplicación, se restauran las pestañas y la pestaña activa.

## Pestañas y navegación

- [ ] Se pueden abrir varios repositorios en pestañas distintas.
- [ ] `Ctrl+Tab` / `Ctrl+Shift+Tab` cambian de pestaña.
- [ ] El botón de cerrar en cada pestaña cierra solo esa sesión.
- [ ] Las pestañas internas `Changes` e `History` cambian sin perder el estado de la otra.

## Vista Changes

- [ ] Los grupos (staged, unstaged, conflictos) muestran contador y se pueden colapsar o expandir.
- [ ] Cada fila muestra código de estado, nombre de archivo y ruta padre truncada si aplica.
- [ ] Al estrechar la ventana, el nombre y la ruta conservan un ancho legible y las acciones bajan
  juntas a una segunda línea sin solaparse con el fichero.
- [ ] `Stage` / `Unstage` por archivo y `Stage todo` / `Unstage todo` actualizan la lista.
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
- [ ] Errores de Git (por ejemplo, pull no fast-forward) aparecen en la barra de estado sin colgar
  la UI.
- [ ] Operaciones largas muestran indicador de carga y se pueden cancelar si aplica.

## Accesibilidad y DPI

- [ ] El texto es legible con escala del sistema al 100 %.
- [ ] Con escala 125 % o 150 %, los botones y filas no se solapan de forma inaceptable.
- [ ] Los colores de estado no son el único indicador (también hay letras/códigos).

## Instalador (si aplica)

- [ ] El MSI instala `git-helper.exe`, `LICENSE` y `THIRD_PARTY_NOTICES.md`.
- [ ] El acceso directo y el icono en “Programas y características” usan `assets/icon.ico`.
- [ ] La desinstalación elimina los archivos instalados sin dejar el ejecutable en `bin`.
