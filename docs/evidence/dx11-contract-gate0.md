# Evidencia de la Puerta 0 — contrato DX11 en CI

| Campo | Valor |
| --- | --- |
| Título | Evidencia de la Puerta 0 del contrato de capacidades DX11 |
| Fecha | 2026-11-01 |
| Puerta | 0 — Congelación y contrato |
| Rama | `dx11-verification` (workflow en `dx11-verification-ci`) |
| Commit base | `b85f3a16a9dfc6bbcc9da650755c23f92b6cf636` |
| Fork | `AndresqDev/wgpu` |
| Workflow | `DX11 Contract` (`.github/workflows/dx11-contract.yml`) |
| Test de contrato | `wgpu-hal/tests/dx11_contract.rs` (se escribe en la rama hermana `dx11-verification-contract`; no está en esta rama) |
| Contrato de referencia | `docs/dx11-capability-contract.md` §9 (repositorio de estabilización de Cinnabar; no vive en este fork de wgpu) |
| Snapshot objetivo | `dx11-gate0-coordination.md` (capa 1), tabla de la Puerta 0 |

## 1. Propósito de este documento

Este documento registra cómo se ejecuta el test de contrato de la Puerta 0 en
GitHub Actions, qué se espera contra el pin actual y qué queda pendiente de
ejecución. **No es un informe de un run ya realizado**: en el momento de
escribirlo no existe ninguna ejecución del workflow `DX11 Contract`. Los campos
de ejecución se dejan explícitamente como `PENDING` y los completa
`lead-verification` (capa 2) cuando el run exista.

Regla que se respeta aquí: no se inventan Run IDs, URLs, nombres de adaptador
ni conclusiones. Cualquier celda sin dato real dice `PENDING`.

## 2. Entorno de ejecución

| Campo | Valor |
| --- | --- |
| Runner | `windows-2025` (GitHub-hosted) |
| GPU | No garantizada. Lo esperado es WARP (renderizador software de D3D) o *Microsoft Basic Render Driver*; puede aparecer hardware en runners con GPU, pero no se asume |
| Toolchain | `dtolnay/rust-toolchain@stable` + componente `rustfmt`; el repo fija `channel = "1.93"` en `rust-toolchain.toml` y la acción respeta ese fichero |
| Feature de Cargo | `dx11` de `wgpu-hal` |
| Variables | `CARGO_INCREMENTAL="0"`, `RUST_BACKTRACE="1"` |
| Permisos | `contents: read` (mínimo privilegio, sin secretos) |
| Timeout del job | 60 minutos |
| Artefacto | `contract-test-log` → `contract-test.log` (`if: always()`, `if-no-files-found: warn`) |

El job se dispara con `push` a `dx11-verification` o `dx11-stabilization`, y con
`workflow_dispatch` mediante la entrada `ref` (por defecto
`dx11-verification`). El checkout usa `ref: ${{ inputs.ref || github.ref }}`:
en `push` la entrada `inputs.ref` está vacía y la expresión cae en
`github.ref`. Eso permite apuntar el mismo workflow a `dx11-hal-core` sin
copiar el fichero a esa rama.

## 3. Estado de la Puerta 0

| Paso del job | Comando | Estado esperado | Estado real |
| --- | --- | --- | --- |
| Check de compilación | `cargo check -p wgpu-hal --features dx11` | Verde | `PENDING — filled by lead-verification after CI` |
| Test de contrato | `cargo test -p wgpu-hal --features dx11 --test dx11_contract -- --nocapture` | **ROJO esperado** contra el pin | `PENDING — filled by lead-verification after CI` |
| Formato | `cargo fmt --all -- --check` | Verde esperado (no verificado) | `PENDING — filled by lead-verification after CI` |

El test de contrato está diseñado para fallar contra el pin actual: el pin
sobre-anuncia capacidades que no implementa. El workflow **no** usa
`continue-on-error` ni debilita ningún paso; el rojo es la evidencia buscada.
Los pasos de check y de formato corren igualmente: el formato lleva
`if: always()` para ejecutarse aunque el test falle.

## 4. Comandos ejecutados en CI

```bash
# 1. Check de compilación del backend con la feature dx11
cargo check -p wgpu-hal --features dx11

# 2. Test de contrato (shell: bash en Windows, pipefail para no ocultar el
#    código de salida de cargo a través de tee)
set -o pipefail
cargo test -p wgpu-hal --features dx11 --test dx11_contract -- --nocapture 2>&1 | tee contract-test.log

# 3. Formato (siempre, incluso si el test de contrato falla)
cargo fmt --all -- --check
```

El workflow no ejecuta `cargo build`, `cargo run` ni ejemplos: la Puerta 0
solo mide anuncio y validación, no draw ni readback.

## 5. Registro de ejecución

Sin ejecuciones todavía. Tabla a completar por `lead-verification` tras el
primer run; se enlaza únicamente la URL del run (sin capturas ni logs crudos,
por tamaño y por no duplicar el artefacto `contract-test-log`).

| Campo | Valor |
| --- | --- |
| Run ID | `PENDING — filled by lead-verification after CI` |
| URL del run | `PENDING — filled by lead-verification after CI` |
| Ref ejecutado (SHA) | `PENDING — filled by lead-verification after CI` |
| Conclusión `cargo check` | `PENDING — filled by lead-verification after CI` |
| Conclusión test `dx11_contract` | `PENDING — filled by lead-verification after CI` |
| Conclusión `cargo fmt --all -- --check` | `PENDING — filled by lead-verification after CI` |
| Artefacto `contract-test-log` | `PENDING — filled by lead-verification after CI` |
| Adaptador (nombre) | `PENDING — filled by lead-verification after CI` |
| Vendor ID / Device ID | `PENDING — filled by lead-verification after CI` |
| Feature level reportado | `PENDING — filled by lead-verification after CI` |
| Software vs hardware | `PENDING — filled by lead-verification after CI` |

## 6. Esperado vs actual contra el pin

Columna «pin» = lo que anuncia `b85f3a16` por inspección de
`wgpu-hal/src/dx11/{conv,adapter,device}.rs`. Columna «objetivo» = snapshot de
la Puerta 0 en el memo de coordinación.

| Aspecto | Pin actual | Objetivo Puerta 0 | Sección del contrato |
| --- | --- | --- | --- |
| Features | `DEPTH_CLIP_CONTROL`, `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`, y `TEXTURE_COMPRESSION_BC` cuando FL ≥ 11_0 | Solo `DEPTH_CLIP_CONTROL` + `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` | §4.1 / §4.3 |
| Downlevel flags | `DownlevelFlags::compliant()` menos 5 flags → conjunto más amplio que el objetivo | Set exacto de 7 flags (`BASE_VERTEX`, `NON_POWER_OF_TWO_MIPMAPPED_TEXTURES`, `CUBE_ARRAY_TEXTURES`, `COMPARISON_SAMPLERS`, `INDEPENDENT_BLEND`, `FULL_DRAW_INDEX_UINT32`, `DEPTH_BIAS_CLAMP`) y `is_webgpu_compliant() == false` | §5 |
| `shader_model` | `Sm5` (FL 11_0) / `Sm4` (10_1) | Igual | §5 |
| Formatos con `MULTISAMPLE_X4` | Todo formato con `RENDER_TARGET` anuncia `MULTISAMPLE_X4` aunque la creación exige `sample_count == 1` | Ningún bit `MULTISAMPLE_*` en ningún formato | §6.2 |
| Formatos BC/ETC2/ASTC/`Stencil8`/`Rgb9e5Ufloat` | `TEXTURE_COMPRESSION_BC` anunciado; `conv::map_texture_format` no mapea BC/ETC2/ASTC/`Stencil8`/`Rgb9e5Ufloat` | Capacidades vacías y fallo explícito en creación de textura | §6.1 / §6.2 |
| Límites de storage | `max_storage_buffers_per_shader_stage = 4` (viene de `Limits::downlevel_defaults()`) | `0` (buffers, texturas, dynamic offsets, binding size) | §7 |
| Límites de compute | `max_compute_workgroup_storage_size = 16352` y demás límites de compute > 0 | Todos `0` | §7 |
| `max_texture_dimension_1d` | Se fija al valor de FL (16384 / 8192) | `0` | §7 |
| `max_texture_dimension_3d` | `2048` | `0` | §7 |
| `max_bindings_per_bind_group` | `1000` (default que arrastra `downlevel_defaults()`) | `64` | §7 |
| Bind group layouts con storage | `create_bind_group_layout` acepta cualquier entrada, incluidas storage buffers/texturas y offsets dinámicos de storage | Error explícito; `set_bind_group` nunca debe aceptar storage y descartarlo en silencio | §8 / §9 |
| Pipeline layout con `immediate_size > 0` | Aceptado y propagado a `PipelineLayout::new` sin validar | Error explícito mientras `max_immediate_size == 0` | §8 |
| Compute / queries / ray tracing | `create_compute_pipeline`, `create_query_set` y las entradas de ray tracing y acceleration structures llaman a `unimplemented!()` | Error explícito, nunca panic | §8 / §9 |
| Superficie | Solo `Bgra8Unorm`/`Bgra8UnormSrgb`, `SRGB`, `COLOR_TARGET`, `Fifo`/`Immediate`, `Opaque` | Igual | §6.3 |
| `Alignments` | `buffer_copy_offset = 1`, `buffer_copy_pitch = 1`, `uniform_bounds_check_alignment = 1` | Fuera del objetivo de la Puerta 0; se mantienen sin cambios y sin verificar | §7 (nota del memo) |

## 7. Fallos esperados de la Puerta 0 y su sección

Lista de fallos que el test `dx11_contract` debe producir contra el pin. Cada
uno corresponde a una aserción del test y a una sección del contrato; un test
que pase contra este pin sería un test roto, no una buena noticia.

| # | Fallo esperado | Sección |
| --- | --- | --- |
| 1 | `TEXTURE_COMPRESSION_BC` anunciado en FL 11_0 y no en el set objetivo | §4.1 / §4.3 |
| 2 | Conjunto de `DownlevelFlags` más amplio que el objetivo y `is_webgpu_compliant()` verdadero por herencia de `compliant()` | §5 |
| 3 | `MULTISAMPLE_X4` presente en formatos con `RENDER_TARGET` | §6.2 |
| 4 | Formatos BC/ETC2/ASTC/`Stencil8`/`Rgb9e5Ufloat` no devuelven capacidades vacías (BC) o no fallan de forma explícita y homogénea | §6.2 |
| 5 | Límites de storage > 0 (`max_storage_buffers_per_shader_stage = 4`, offsets dinámicos de storage) | §7 |
| 6 | Límites de compute > 0 (`max_compute_workgroup_storage_size`, invocaciones, workgroups por dimensión) | §7 |
| 7 | `max_texture_dimension_1d = 16384/8192` y `max_texture_dimension_3d = 2048` en vez de `0` | §7 |
| 8 | `max_bindings_per_bind_group = 1000` en vez de `64` | §7 |
| 9 | `create_bind_group_layout` acepta storage buffers/texturas y offsets dinámicos de storage | §8 |
| 10 | `create_pipeline_layout` acepta `immediate_size = 4` con `max_immediate_size == 0` | §8 |
| 11 | `create_compute_pipeline`, `create_query_set` y las entradas de ray tracing panican con `unimplemented!()` en vez de devolver error explícito | §8 |
| 12 | Texturas 1D/3D, MSAA con `sample_count > 1` y view formats arbitrarios no fallan todavía con error explícito y uniforme | §8 / §9 (positivos/negativos) |

## 8. Consecuencia operativa (no oculta)

Con `max_storage_buffers_per_shader_stage = 0`, `max_storage_textures_per_shader_stage = 0`,
los límites de compute a `0` y `max_bindings_per_bind_group = 64`,
`wgpu-core` valida `required_limits` contra los límites del adaptador en
`request_device`. Por tanto:

- un `request_device` que pida `Limits::downlevel_defaults()` **o**
  `Limits::default()` será **rechazado** (piden storage/compute > 0 y
  `max_bindings_per_bind_group = 1000`);
- el perfil `Dx11Legacy` de Cinnabar debe pedir los límites del adaptador o un
  conjunto explícito bajo, no los defaults;
- el test de contrato abre el dispositivo HAL con **los límites del propio
  adaptador**, no con `downlevel_defaults()`.

Esto es el precio honesto de no anunciar storage ni compute que el backend no
implementa. Se documenta aquí y en el contrato; no se esconde relajando el
test ni subiendo los límites del adaptador.

## 9. Limitaciones de esta evidencia

- **Sin GPU de hardware garantizada.** El runner `windows-2025` puede ejecutar
  solo WARP / *Microsoft Basic Render Driver*. Las diferencias entre software y
  hardware se documentan como limitación; el contrato §9.7 pide pasar en ambos
  y aquí solo se cubre el que el runner ofrezca.
- **`maximum_frame_latency` no se aplica en `configure`.** No se verifica; se
  registra como limitación (memo de coordinación, superficie).
- **`Alignments` sin cambios y sin verificar.** `buffer_copy_offset`,
  `buffer_copy_pitch` y `uniform_bounds_check_alignment` valen `1` en el pin y
  quedan como seguimiento posterior a la Puerta 0.
- **Sin draw ni readback en la Puerta 0.** El test mide anuncio y validación;
  la conformidad de dibujo, copias y readback pertenece a la Puerta 1.
- **Ruta de superficie solo Win32 `HWND`.** No hay Wayland/X11/Metal; la
  superficie se valida únicamente en el camino Win32.
- **El test no está en esta rama.** `wgpu-hal/tests/dx11_contract.rs` se
  escribe en `dx11-verification-contract`; el workflow se escribió contra esa
  ruta final. Hasta que ambas ramas se junten, un `workflow_dispatch` sobre
  esta rama fallará en el paso del test por *test target* inexistente, no por
  contrato. Ese fallo no es evidencia de la Puerta 0.
- **Sin capturas ni logs crudos en el documento.** Se enlaza la URL del run
  (placeholder) y el artefacto `contract-test-log`.

## 10. Próximo paso

1. P1 (`lead-hal-core`) y P2 corrigen el backend al snapshot objetivo: features,
   downlevel, formatos, límites, validación explícita de bind group layouts,
   pipeline layouts y stubs de compute/queries/ray tracing.
2. `lead-verification` ejecuta `DX11 Contract` sobre la rama que tenga el test
   y rellena la tabla de la sección 5 con Run ID, URL, conclusiones, adaptador y
   feature level reales.
3. El test de contrato debe quedar **verde** contra el snapshot objetivo; solo
   entonces la Puerta 0 puede cerrarse y el contrato actualizarse.
4. Mientras siga rojo, este workflow y el test **no se mergean** en
   `dx11-stabilization` ni en `dx11-integration`. Un rojo no se integra.
