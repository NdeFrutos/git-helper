# Assets de Git Helper

## Icono (`icon.ico`)

Icono propio de Git Helper, generado con OpenAI ImageGen el 10 de septiembre de 2026. Representa
un flujo de ramas Git sobre una base azul marino y no incorpora marcas de terceros.

- `icon.png`: fuente raster de alta resolución.
- `icon.ico`: paquete multirresolución para el ejecutable, el instalador MSI y los accesos directos
  de Windows.

El recurso se incrusta en el ejecutable como icono `1` desde `build.rs`, que es el identificador que
GPUI carga al crear la ventana nativa.
