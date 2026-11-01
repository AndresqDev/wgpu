# Evidencia de la Puerta 0 — contrato DX11 en CI

| Campo | Valor |
| --- | --- |
| Título | Evidencia de la Puerta 0 del contrato de capacidades DX11 |
| Fecha | 2026-11-01 |
| Puerta | 0 — Congelación y contrato |
| Rama del test y del workflow | `dx11-verification` |
| Commit base (pin) | `b85f3a16a9dfc6bbcc9da650755c23f92b6cf636` |
| Fork | `AndresqDev/wgpu` |
| Workflow | `DX11 Contract` (`.github/workflows/dx11-contract.yml`) |
| Test de contrato | `wgpu-hal/tests/dx11_contract.rs` (incluido en esta rama) |
| Contrato de referencia | `docs/dx11-capability-contract.md` §9 (repositorio de estabilización de Cinnabar; no vive en este fork de wgpu) |
| Snapshot objetivo | `dx11-gate0-coordination.md` (capa 1), tabla de la Puerta 0 |
| Candidato de integración | Rama `dx11-gate0-candidate`, commit `1deb0295817d057dd18bb22597bf8d334b4ff909` (merge de `10115febd` + P1 `640c64917`) |
| Estado | Contrato **VERDE** en el candidato P6+P1. Solo software/WARP en el runner; la paridad con hardware no está verificada |

## 1. Propósito y alcance

Este documento registra dos ejecuciones reales del workflow `DX11 Contract`:

1. **Run rojo del pin:** el commit P6 `10115febd` (test y workflow, sin las
   correcciones de P1), basado en el pin `b85f3a16a`. El test falla de forma
   esperada y enumera los sobre-anuncios del backend.
2. **Run verde del candidato:** el commit `1deb02958`, merge de `10115febd` con
   la corrección de P1 `640c64917`. El mismo test pasa sin cambios ni
   debilitamientos.

No quedan campos pendientes: todos los Run ID, URLs, adaptadores y
conclusiones de este documento son datos observados en GitHub Actions. El run
verde demuestra el contrato **solo en el renderizador software** que ofrece el
runner; **no** se afirma paridad con hardware.

## 2. Entorno de ejecución

| Campo | Valor |
| --- | --- |
| Runner | `windows-2025` (GitHub-hosted) |
| Adaptadores observados | Dos adaptadores idénticos `Microsoft Basic Render Driver`, vendor `0x1414`, device `0x008c`, feature level `11_0` |
| GPU | Software (`Microsoft Basic Render Driver`). No apareció ningún adaptador de hardware |
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
`github.ref`. Eso permite apuntar el mismo workflow al candidato sin copiar el
fichero a esa rama.

**Limitación de clasificación software:** el test imprime `software/WARP:
false` porque usa `wgt::DeviceType::Cpu` para detectar software, y el
`Microsoft Basic Render Driver` se clasifica como `IntegratedGpu`. El nombre del
adaptador y la ausencia de hardware confirman que es software. Es un punto de
endurecimiento del test, no una prueba de hardware.

## 3. Estado de la Puerta 0

| Paso del job | Comando | Pin P6-only `10115febd` (run 41306698696) | Candidato P6+P1 `1deb02958` (run 41308541042) |
| --- | --- | --- | --- |
| Check de compilación | `cargo check -p wgpu-hal --features dx11` | ✓ | ✓ |
| Test de contrato | `cargo test -p wgpu-hal --features dx11 --test dx11_contract -- --nocapture` | ✗ **ROJO esperado**: 146 violaciones (~73 por adaptador) | ✓ **1 passed; 0 failed** |
| Formato | `cargo fmt --all -- --check` | ✓ | ✓ |
| Artefacto `contract-test-log` | `actions/upload-artifact@v4` | ✓ | ✓ |

El run rojo es la evidencia buscada: el workflow **no** usa
`continue-on-error` ni debilita ningún paso. El run verde usa el mismo test,
sin cambios, sobre el candidato que incluye las correcciones de P1. El formato
lleva `if: always()` para ejecutarse aunque el test falle.

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

## 5. Registro de ejecuciones

| Campo | Run rojo del pin (P6-only) | Run verde del candidato (P6+P1) |
| --- | --- | --- |
| Run ID | `41306698696` | `41308541042` |
| URL del run | https://github.com/AndresqDev/wgpu/actions/runs/41306698696 | https://github.com/AndresqDev/wgpu/actions/runs/41308541042 |
| Ref ejecutado | `10115febd` (test+workflow, base pin `b85f3a16a`) | `1deb0295817d057dd18bb22597bf8d334b4ff909` (rama `dx11-gate0-candidate`) |
| Disparo | `push` a `dx11-verification` | `workflow_dispatch` con `ref: dx11-gate0-candidate` |
| Runner | `windows-2025` | `windows-2025` |
| Conclusión `cargo check` | ✓ | ✓ |
| Conclusión test `dx11_contract` | ✗ esperado: 146 violaciones (~73 por adaptador) | ✓ 1 passed; 0 failed |
| Conclusión `cargo fmt --all -- --check` | ✓ | ✓ |
| Artefacto `contract-test-log` | ✓ | ✓ |
| Adaptador (nombre) | Dos × `Microsoft Basic Render Driver` | Dos × `Microsoft Basic Render Driver` |
| Vendor ID / Device ID | `0x1414` / `0x008c` | `0x1414` / `0x008c` |
| Feature level reportado | `11_0` | `11_0` |
| Software vs hardware | Software. El flag impreso `software/WARP` fue `false` por la limitación de `DeviceType::Cpu` descrita en §2; no es hardware | Software. Misma limitación de clasificación; no es hardware |
| Nota | El run rojo es de la rama P6-only, antes de las correcciones de P1 | El run verde es el merge P1+P6; el checkout del job confirma `1deb02958` |

Se enlaza únicamente la URL de cada run, sin capturas ni logs crudos, para no
duplicar el artefacto `contract-test-log`.

## 6. Esperado vs actual contra el pin (run rojo)

Columna «pin» = lo que anunciaba `b85f3a16` por inspección de
`wgpu-hal/src/dx11/{conv,adapter,device}.rs` y lo observado en el run rojo.
Columna «objetivo» = snapshot de la Puerta 0 en el memo de coordinación. El
candidato P6+P1 cumple el objetivo en todas estas filas; la verificación verde
detallada está en §7.

| Aspecto | Pin actual (run rojo) | Objetivo Puerta 0 | Sección del contrato |
| --- | --- | --- | --- |
| Features | `DEPTH_CLIP_CONTROL`, `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`, y `TEXTURE_COMPRESSION_BC` cuando FL ≥ 11_0 | Solo `DEPTH_CLIP_CONTROL` + `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` | §4.1 / §4.3 |
| Downlevel flags | `DownlevelFlags::compliant()` menos 5 flags → 13 flags extra (aunque `is_webgpu_compliant()` ya era `false` en el pin) | Set exacto de 7 flags (`BASE_VERTEX`, `NON_POWER_OF_TWO_MIPMAPPED_TEXTURES`, `CUBE_ARRAY_TEXTURES`, `COMPARISON_SAMPLERS`, `INDEPENDENT_BLEND`, `FULL_DRAW_INDEX_UINT32`, `DEPTH_BIAS_CLAMP`) y `is_webgpu_compliant() == false` | §5 |
| `shader_model` | `Sm5` (FL 11_0) / `Sm4` (10_1) | Igual | §5 |
| Formatos con `MULTISAMPLE_X4` | 37 formatos con `RENDER_TARGET` anuncian `MULTISAMPLE_X4` aunque la creación exige `sample_count == 1` | Ningún bit `MULTISAMPLE_*` en ningún formato | §6.2 |
| Formatos BC/ETC2/ASTC/`Stencil8`/`Rgb9e5Ufloat` | `TEXTURE_COMPRESSION_BC` anunciado; las capacidades por formato de estos formatos sí eran vacías en el pin, pero la feature permitía pedirlos | Capacidades vacías y fallo explícito en creación de textura | §6.1 / §6.2 |
| Límites de storage | `max_storage_buffers_per_shader_stage = 4` (viene de `Limits::downlevel_defaults()`), texturas de storage y offsets dinámicos de storage > 0 | `0` (buffers, texturas, dynamic offsets, binding size) | §7 |
| Límites de compute | `max_compute_workgroup_storage_size = 16352` y demás límites de compute > 0 | Todos `0` | §7 |
| `max_texture_dimension_1d` | `16384` / `8192` según FL | `0` | §7 |
| `max_texture_dimension_3d` | `2048` | `0` | §7 |
| `max_bindings_per_bind_group` | `1000` (default que arrastra `downlevel_defaults()`) | `64` | §7 |
| `max_buffers_and_acceleration_structures_per_shader_stage` | `24` | `12` | §7 |
| Bind group layouts con storage | `create_bind_group_layout` acepta storage buffers/texturas y offsets dinámicos de storage | Error explícito; `set_bind_group` nunca debe aceptar storage y descartarlo en silencio | §8 / §9 |
| Pipeline layout con `immediate_size > 0` | Aceptado y propagado a `PipelineLayout::new` sin validar | Error explícito mientras `max_immediate_size == 0` | §8 |
| Compute / queries / ray tracing | `create_compute_pipeline`, `create_query_set`, `create_ray_tracing_pipeline` y `create_acceleration_structure` llaman a `unimplemented!()` | Error explícito, nunca panic | §8 / §9 |
| Superficie | Solo `Bgra8Unorm`/`Bgra8UnormSrgb`, `SRGB`, `COLOR_TARGET`, `Fifo`/`Immediate`, `Opaque` | Igual | §6.3 |
| `Alignments` | `buffer_copy_offset = 1`, `buffer_copy_pitch = 1`, `uniform_bounds_check_alignment = 1` | Fuera del objetivo de la Puerta 0; se mantienen sin cambios y sin verificar | §7 (nota del memo) |

El run rojo reportó **146 violaciones, ~73 por adaptador**. El desglose por
adaptador fue: 1 feature extra, 13 downlevel flags extra, 14 límites fuera del
objetivo, 37 formatos con `MULTISAMPLE_X4`, 3 layouts aceptados que debían
fallar y 4 `unimplemented!()` que debían devolver error. El recuento bruto
incluye además un aviso duplicado de bits «unnamed» por adaptador, que no
altera el diagnóstico.

## 7. Verificación del candidato verde (P6+P1)

> **Nota de superación (2026-11-01).** La rama `dx11-storage-readonly`
> (base `47057539f`) sustituye el objetivo histórico de storage de esta
> sección: ahora se anuncian storage buffers **read-only** en FL 11_0 con
> `max_storage_buffers_per_shader_stage = 4`,
> `max_storage_buffer_binding_size = 128 MiB`,
> `VERTEX_STORAGE|FRAGMENT_STORAGE` y `min_storage_buffer_offset_alignment =
> 256`. Compute, UAV/read-write, storage textures, offsets dinámicos y arrays
> siguen fuera. Las filas y datos históricos de los runs de Gate 0 que
> aparecen debajo no se modifican; la nueva evidencia está en
> [`dx11-storage-readonly.md`](./dx11-storage-readonly.md).

Run `41308541042`, commit `1deb02958`, `windows-2025`. El paso
`cargo test -p wgpu-hal --features dx11 --test dx11_contract -- --nocapture`
terminó con **1 passed; 0 failed; 0 ignored** (0.04 s). El mismo test que falló
en el pin pasó sin cambios. Comprobaciones verdes observadas:

- **Features:** exactamente `DEPTH_CLIP_CONTROL |
  TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`. `TEXTURE_COMPRESSION_BC` ya no se
  anuncia.
- **Downlevel:** exactamente las 7 flags del objetivo;
  `is_webgpu_compliant() == false`; `shader_model = Sm5` en FL `11_0`
  (`Sm4` para FL `10_1` no se observó porque el runner solo expuso `11_0`).
- **Límites:** exactamente la tabla de la Puerta 0. En particular,
  `max_texture_dimension_1d = 0`, `max_texture_dimension_3d = 0`, todos los
  límites de storage y compute a `0`, `max_bindings_per_bind_group = 64`,
  `max_buffers_and_acceleration_structures_per_shader_stage = 12` y
  `max_texture_dimension_2d = 16384` en FL `11_0`.
- **Formatos:** ningún formato anuncia `MULTISAMPLE_*`; BC1–BC7, ETC2/EAC,
  ASTC, `Stencil8` y `Rgb9e5Ufloat` devuelven capacidades vacías; la allowlist
  de §6.1 se mantiene.
- **Negativos rechazados:** texturas BC (`Bc1RgbaUnorm`, `Bc7RgbaUnormSrgb`),
  texturas MSAA (`sample_count = 4`), render pass MSAA, render pipeline MSAA,
  texturas 1D y 3D, view format arbitrario (`Rgba8Unorm` + `Rgba8Uint`),
  bind group layout con storage buffer, bind group layout con storage texture
  y pipeline layout con `immediate_size = 4`.
- **Descriptor entry points devuelven `Err`, sin panic:**
  `create_compute_pipeline`, `create_query_set`,
  `create_ray_tracing_pipeline` y `create_acceleration_structure`.
- **Superficie:** capacidades exactas (`Bgra8Unorm`/`Bgra8UnormSrgb`, `SRGB`,
  `COLOR_TARGET`, `Fifo`/`Immediate`, `Opaque`); `configure` rechaza
  `Rgba8Unorm` y `Mailbox`.
- **Smoke positivo:** buffers vertex/index/uniform/copy, textura 2D con view,
  sampler, bind group con uniform buffer, shader module y render pipeline
  creados correctamente. Sin draw ni readback (Puerta 1).

**Alcance del verde:** este run demuestra el contrato en el renderizador
software `Microsoft Basic Render Driver` del runner. No demuestra paridad con
hardware ni sustituye la validación en la Intel HD 4400.

## 8. Consecuencia operativa (no oculta)

Con `max_storage_buffers_per_shader_stage = 0`,
`max_storage_textures_per_shader_stage = 0`, los límites de compute a `0` y
`max_bindings_per_bind_group = 64`, `wgpu-core` valida `required_limits` contra
los límites del adaptador en `request_device`. Por tanto:

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

- **Solo software, sin hardware.** El runner `windows-2025` expuso únicamente
  `Microsoft Basic Render Driver` (`0x1414:0x008c`, FL `11_0`). No hubo
  adaptador de hardware. La paridad software/hardware que pide el contrato
  §9.7 sigue abierta y no se afirma.
- **Detección software incompleta en el test.** El flag `software/WARP`
  imprimió `false` porque `DeviceType::Cpu` no clasifica el
  `Microsoft Basic Render Driver`. El nombre del adaptador y la ausencia de
  hardware lo identifican como software; es un punto de endurecimiento, no una
  capacidad de hardware.
- **Dos adaptadores idénticos.** DXGI enumeró dos veces el mismo
  `Microsoft Basic Render Driver`; el test ejecutó las comprobaciones en ambos.
  No cambia el resultado, pero conviene deduplicar o etiquetar mejor en una
  revisión futura del harness.
- **`maximum_frame_latency` no se aplica en `configure`.** No se verifica; se
  registra como limitación (memo de coordinación, superficie).
- **`Alignments` sin cambios y sin verificar.** `buffer_copy_offset`,
  `buffer_copy_pitch` y `uniform_bounds_check_alignment` valen `1` en el pin y
  quedan como seguimiento posterior a la Puerta 0.
- **Ray tracing en el command encoder.** Los métodos de ray tracing del
  command encoder son no-ops silenciosos en este pin. El test no los marca
  como fallo porque los descriptor entry points (`create_ray_tracing_pipeline`,
  `create_acceleration_structure`) fallan primero y bloquean la alcanzabilidad.
- **`request_device` con defaults.** Con storage/compute a `0` y
  `max_bindings_per_bind_group = 64`, `wgpu-core` rechaza
  `Limits::downlevel_defaults()`/`Limits::default()`. El perfil `Dx11Legacy`
  debe pedir los límites del adaptador; no es un defecto del contrato.
- **Dos runs, dos ramas.** El run rojo es de la rama P6-only `10115febd`
  (basada en el pin); el verde es el merge P1+P6 `1deb02958` en
  `dx11-gate0-candidate`. No son el mismo commit y no se deben confundir.
- **Sin draw ni readback en la Puerta 0.** El test mide anuncio y validación;
  la conformidad de dibujo, copias y readback pertenece a la Puerta 1.
- **Ruta de superficie solo Win32 `HWND`.** No hay Wayland/X11/Metal; la
  superficie se valida únicamente en el camino Win32.
- **Sin capturas ni logs crudos en el documento.** Se enlazan las URLs de los
  runs y el artefacto `contract-test-log`.

## 10. Próximo paso

1. La capa 1 revisa e integra el candidato `dx11-gate0-candidate` (`1deb02958`)
   en la rama de integración. Este documento no mergea ni publica nada.
2. La Puerta 0 queda verde en software/WARP. La validación en hardware
   (Intel HD 4400) sigue pendiente y no debe declararse cerrada por este run.
3. Cualquier cambio futuro de features, downlevel, límites, formatos o
   superficie debe pasar de nuevo `DX11 Contract`; el snapshot de la Puerta 0
   es el contrato, no una conveniencia del test.
4. El run rojo no se integra. El candidato verde es el que puede revisar la
   capa 1; ninguna build de usuario se publica hasta que se cumplan las puertas
   correspondientes.
