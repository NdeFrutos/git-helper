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
- [ ] Al cerrar y volver a abrir la aplicación, se restauran las pestañas, la pestaña activa, la
  geometría de ventana y el borrador del mensaje de commit.
- [ ] Un commit fallido conserva el borrador; un commit exitoso lo limpia.
- [ ] Si un repositorio guardado no está accesible (disco desconectado), la pestaña permanece con
  aviso, botón Reintentar y Cerrar; al recuperar la ruta, el estado vuelve a cargarse.
- [ ] El estado vacío muestra repositorios recientes persistidos.

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
