# Avisos de terceros

## GPUI

Git Helper usa GPUI y `gpui_platform` de Zed Industries, Inc., fijados al commit
`5896274168e06663568f42b5bae142162936e0a6`.

GPUI se distribuye bajo Apache License 2.0:
<https://www.apache.org/licenses/LICENSE-2.0>.

Copyright 2022-2025 Zed Industries, Inc.

`src/ui/commit_input.rs` adapta el patrón de `crates/gpui/examples/input.rs`; se han eliminado el
layout de línea y el pintado del caret, y se ha añadido entrada multilínea orientada al mensaje de
commit.

Los crates `git_ui`, `git`, `workspace`, `editor` y `ui` de Zed se revisaron como referencia, pero no
se compilan, enlazan ni copian porque se distribuyen bajo GPL-3.0-or-later.

## Licencia de Git Helper

Git Helper se distribuye bajo Apache License 2.0. El texto completo está en `LICENSE`.

## Dependencias de Rust

Las licencias de las dependencias transitivas de la compilación release se generan con:

```powershell
cargo license --avoid-dev-deps --avoid-build-deps --do-not-bundle > docs\dependency-licenses.txt
```

El listado actual se incluye en `docs/dependency-licenses.txt` y debe regenerarse antes de cada
publicación de binarios o MSI.
