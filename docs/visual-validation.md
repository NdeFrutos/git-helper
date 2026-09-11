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
- [ ] La inspección interactiva de 960×640 y DPI 100/125/150/200 % queda pendiente cuando haya un
  entorno de captura nativa disponible; el conector usado en esta revisión no expuso ventanas de
  aplicaciones de escritorio.
