# Empaquetado para Windows

## Requisitos

- Los requisitos de compilación descritos en `README.md`.
- [WiX Toolset 3.14](https://wixtoolset.org/docs/wix3/).
- `cargo-wix` 0.3.9.
- `cargo-license` para regenerar `docs/dependency-licenses.txt`.

Git Helper usa Apache-2.0 (`LICENSE`). La plantilla WiX versionada está en `wix/main.wxs`.

## Procedimiento reproducible

```powershell
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo build --release --locked

cargo license --avoid-dev-deps --avoid-build-deps --do-not-bundle > docs\dependency-licenses.txt

cargo install cargo-wix --version 0.3.9 --locked
cargo wix --nocapture
```

Si WiX no está en `PATH`, instalar el toolset y reiniciar la consola:

```powershell
winget install --id WiXToolset.WiXToolset --exact
```

El MSI resultante se crea en `target\wix`. Incluye:

- `target\release\git-helper.exe`.
- `LICENSE`.
- `THIRD_PARTY_NOTICES.md`.
- Icono propio multirresolución `assets/icon.ico`, compartido por el ejecutable y el instalador.

La configuración de empaquetado vive en `[package.metadata.wix]` dentro de `Cargo.toml`. Los GUID de
actualización y de `PATH` están fijados para permitir upgrades in-place.

Los artefactos actuales no se firman con Authenticode. Las releases incluyen `SHA256SUMS.txt` para
comprobar su integridad; la firma queda pendiente para una fase posterior:

```powershell
cargo wix sign
```

## Publicación automatizada

El workflow `.github/workflows/release.yml` ejecuta formato, Clippy, pruebas, build release y
`cargo-wix` en un runner oficial de Windows. Después publica el MSI, un ZIP portable y sus hashes
SHA-256 en GitHub Releases. Se activa al subir una etiqueta `vMAJOR.MINOR.PATCH` o manualmente desde
GitHub Actions, y exige que la versión coincida con `Cargo.toml`.

## Validación posterior al empaquetado

Seguir el checklist de [`docs/visual-validation.md`](visual-validation.md) en el binario release y,
si procede, tras instalar el MSI.
