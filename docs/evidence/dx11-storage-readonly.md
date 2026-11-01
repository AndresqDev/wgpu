# DX11 storage buffers read-only en WARP

| Campo | Valor |
| --- | --- |
| Fecha | 2026-11-01 |
| Rama | `dx11-storage-readonly` |
| Base | `47057539f` (`Merge DX11 query pool follow-up`) |
| Commit de código | `PENDIENTE` (se fijará al cerrar el paquete; este documento no se ha ejecutado todavía) |
| Runner | GitHub Actions `windows-2025` |
| Adaptador | Microsoft Basic Render Driver (WARP), feature level 11_0 |
| Workflow | `DX11 storage check` (`.github/workflows/dx11-storage-check.yml`) |
| Runs | `PENDIENTE` (sin ejecución todavía; el líder debe lanzar el workflow y rellenar run IDs y log) |
| Tests | `dx11_storage` (nuevo), `dx11_contract`, `dx11_descriptors`, `dx11_resources` y `--lib` |
| Estado | Implementación y tests preparados. La evidencia de ejecución queda pendiente de CI |

## Cambio implementado

El backend DX11 deja de rechazar todos los buffers de storage y acepta
**storage buffers de solo lectura** en feature level 11_0:

- `Device::create_buffer` acepta `STORAGE_READ_ONLY`, también cuando viene
  acompañado de `STORAGE_READ_WRITE` porque `wgpu-core` mapea
  `BufferUsages::STORAGE` a ambos bits. Rechaza `STORAGE_READ_WRITE` sin
  `STORAGE_READ_ONLY`, y cualquier combinación con
  `MAP_READ`/`MAP_WRITE`/`VERTEX`/`INDEX`/`UNIFORM`/`INDIRECT`.
- El buffer se crea con `D3D11_BIND_SHADER_RESOURCE` y
  `D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS`.
- `BindGroupLayout` acepta `BufferBindingType::Storage { read_only: true }`
  con `has_dynamic_offset: false`, sin arrays y con visibilidad limitada a
  `VERTEX`/`FRAGMENT`. Rechaza storage read-write/UAV, offsets dinámicos,
  arrays, storage textures y visibilidad `COMPUTE`.
- `PipelineLayout` asigna al binding un registro `t` plano, compartido con las
  texturas muestreadas.
- `BindGroup` crea un SRV raw (`DXGI_FORMAT_R32_TYPELESS`,
  `D3D_SRV_DIMENSION_BUFFEREX`, `D3D11_BUFFEREX_SRV_FLAG_RAW`) por binding. El
  offset estático se hornea en `FirstElement` y el tamaño en `NumElements`, así
  que no hace falta `ID3D11DeviceContext1` ni offsets dinámicos.
- `CommandEncoder::set_bind_group` enlaza el SRV con
  `VSSetShaderResources`/`PSSetShaderResources` en el registro calculado por el
  pipeline layout. Los buffers uniform siguen usando constant buffers.

Naga traduce `var<storage, read>` a `ByteAddressBuffer`, que es exactamente lo
que el SRV raw expone. El shader de prueba usa `data.value.x` y el test
comprueba el HLSL generado.

## Capacidades anunciadas

Solo en feature level 11_0:

| Capacidad | Valor |
| --- | --- |
| `DownlevelFlags::VERTEX_STORAGE` | presente |
| `DownlevelFlags::FRAGMENT_STORAGE` | presente |
| `max_storage_buffers_per_shader_stage` | `4` |
| `max_storage_buffer_binding_size` | `128 << 20` (128 MiB) |
| `min_storage_buffer_offset_alignment` | `256` |
| `max_dynamic_storage_buffers_per_pipeline_layout` | `0` |
| `max_storage_textures_per_shader_stage` | `0` |
| `max_buffers_and_acceleration_structures_per_shader_stage` | `12` (sin cambios) |

En feature level 10_1 no se anuncia ninguno de los dos flags de storage y los
límites de storage siguen a `0`. D3D11 solo garantiza `ByteAddressBuffer` de
forma opcional en 10_x, por lo que el backend no lo promete allí.

## Lo que NO se anuncia

- `FRAGMENT_WRITABLE_STORAGE`: no hay UAV ni storage escribible.
- `COMPUTE_SHADERS`: no hay compute pipelines ni dispatches.
- `BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED`: los rangos de binding deben respetar
  `min_storage_buffer_offset_alignment = 256` y múltiplos de 4 bytes para el
  SRV raw.
- Storage textures de cualquier acceso.
- Storage buffers read-write/UAV.
- Offsets dinámicos de storage (`has_dynamic_offset: true`).
- Arrays de bindings de storage (`count > 1`).
- Visibilidad `COMPUTE` para bindings de storage.
- Storage en feature level 10_1.

## Cobertura real por archivo

`wgpu-hal/tests/dx11_storage.rs` (nuevo, end-to-end offscreen):

- **Capacidades:** en FL 11_0 comprueba `VERTEX_STORAGE|FRAGMENT_STORAGE`,
  `max_storage_buffers_per_shader_stage == 4`,
  `max_storage_buffer_binding_size == 128 << 20`,
  `max_dynamic_storage_buffers_per_pipeline_layout == 0`,
  `max_storage_textures_per_shader_stage == 0` y
  `min_storage_buffer_offset_alignment == 256`. Comprueba que
  `FRAGMENT_WRITABLE_STORAGE`, `COMPUTE_SHADERS` y
  `BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED` no están anunciados. En FL 10_1
  comprueba la ausencia de storage, imprime y omite las secciones GPU.
- **Creación de buffers:** acepta
  `STORAGE_READ_ONLY|STORAGE_READ_WRITE|COPY_DST`; rechaza
  `STORAGE_READ_WRITE` puro, `STORAGE_READ_ONLY|MAP_WRITE` y
  `STORAGE_READ_ONLY|VERTEX`.
- **Layouts:** acepta el layout read-only visible `VERTEX_FRAGMENT` con
  `min_binding_size = 16` y comprueba que el binding cae en `t0`; rechaza
  read-write, offset dinámico, array (`count: Some(2)`), visibilidad `COMPUTE`
  y storage texture.
- **Shader Naga/HLSL:** WGSL con `struct Data { value: vec4<u32> }`,
  `@group(0) @binding(0) var<storage, read> data: Data;`, vertex fullscreen con
  `@builtin(vertex_index)` y fragment que devuelve
  `vec4<f32>(f32(data.value.x) / 255.0, 0.0, 0.0, 1.0)`. Compila ambos entry
  points, imprime el HLSL y exige `ByteAddressBuffer`, ausencia de
  `RWByteAddressBuffer` y `register(t0)` (tolera `space0`). Crea el
  `RenderPipeline` real.
- **Datos y subrango:** buffer de 512 bytes con usage storage + `COPY_DST`;
  staging `MAP_WRITE|COPY_SRC` de 16 bytes con `255u32` little-endian en
  `0..4`; `copy_buffer_to_buffer` a offset 256; bind group storage con offset
  256 y size 16; `validate_bind_group`; `set_bind_group`; draw fullscreen a una
  textura `Rgba8Unorm` 4x4 `COLOR_TARGET|COPY_SRC`; readback con
  `copy_texture_to_buffer` y buffer `MAP_READ|COPY_DST`; píxeles `(0,0)` y
  `(3,3)` iguales a `[255, 0, 0, 255]`. Si el offset estático se ignorara, el
  shader leería el cero de `0..4` y el píxel sería negro.
- **Negativos de bind group:** un layout storage read-only con un buffer sin
  usage storage (`COPY_DST`) falla al crear el bind group.

`wgpu-hal/tests/dx11_contract.rs`:

- Los flags y límites esperados dependen del feature level: en 11_0 añade
  `VERTEX_STORAGE|FRAGMENT_STORAGE`, `max_storage_buffers_per_shader_stage = 4`
  y `max_storage_buffer_binding_size = 128 << 20`; en 10_1 exige que no
  aparezcan y que los límites sigan a `0`.
- Comprueba explícitamente que `FRAGMENT_WRITABLE_STORAGE`,
  `COMPUTE_SHADERS` y `BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED` no están
  anunciados.
- Nuevo `positive_storage_smoke` (solo FL 11_0): bind group layout read-only,
  pipeline layout, buffer de 512 bytes con
  `STORAGE_READ_ONLY|STORAGE_READ_WRITE|COPY_DST`, bind group con offset 256 y
  size 16, `set_bind_group` y `end_encoding`.
- El negativo de layout de storage read-only se sustituye por read-write y
  offset dinámico. Las comprobaciones de formatos sin storage/multisample y de
  storage textures se mantienen sin debilitar.

`wgpu-hal/tests/dx11_descriptors.rs`:

- Nueva sección positiva de storage: layout read-only visible
  `VERTEX_FRAGMENT` con `min_binding_size = Some(16)`, pipeline layout, binding
  en `t0`, buffer de 512 bytes con
  `STORAGE_READ_ONLY|STORAGE_READ_WRITE|COPY_DST`, bind groups con offset
  256/size 16 y offset 0/size 64, `validate_bind_group` y `set_bind_group` con
  `end_encoding`.
- Negativo de bind group con un buffer sin usage storage.
- La sección de layouts negativos ya no incluye storage read-only; mantiene
  read-write y añade offset dinámico, array de storage y visibilidad `COMPUTE`.
  El negativo de storage texture se mantiene.

`wgpu-hal/tests/dx11_resources.rs`:

- Positivos nuevos: `STORAGE_READ_ONLY|COPY_DST` y
  `STORAGE_READ_ONLY|STORAGE_READ_WRITE|COPY_DST` (el mapeo de `wgpu-core`).
- Negativos: se retira "storage read-only"; se mantiene
  `STORAGE_READ_WRITE` sin `STORAGE_READ_ONLY` y se añaden storage +
  `MAP_WRITE`, storage + `VERTEX`, storage + `UNIFORM` y storage + `INDIRECT`.
- El resto de casos de buffers, mapping, copias, texturas, arrays, cubemaps,
  formatos, views y samplers no cambia.

`wgpu-hal/src/dx11/mod.rs`: el docstring del módulo describe los storage
buffers read-only vía raw SRV y mantiene compute/UAV/storage textures fuera.
Los demás archivos del HAL no se tocan en este paquete.

## Limitaciones

- **Solo WARP.** El runner `windows-2025` expone `Microsoft Basic Render
  Driver` (FL 11_0). No hay ejecución en hardware en esta evidencia, así que no
  se afirma paridad con la Intel HD 4400.
- **Sin Cinnabar ni Bevy.** El paquete valida la capa HAL; no ejecuta el
  frontend, ni ejemplos con ventana, ni ANGLE/GL.
- **Sin compute.** No se prueban dispatches, UAV, storage textures ni
  read-write; el backend los rechaza explícitamente.
- **Sin offsets dinámicos ni arrays.** Solo se cubre el offset estático
  horneado en el SRV; `min_storage_buffer_offset_alignment` se comprueba en
  los negativos de layout, no con una matriz exhaustiva de offsets.
- **Un solo pipeline y una sola textura de destino.** La cobertura end-to-end
  usa un shader, un binding `t0`, un rango de 16 bytes y una textura 4x4. No
  cubre varios storage buffers simultáneos, aunque el límite anunciado sea 4.
- **HLSL dependiente de Naga/FXC.** El test comprueba el texto generado
  (`ByteAddressBuffer`, `register(t0)`) y compila con FXC, pero no fija la
  versión exacta de Naga ni el formato completo del HLSL.
- **La evidencia de CI está pendiente.** No se ha lanzado el workflow desde
  este paquete; los run IDs y el log deben añadirse tras la ejecución del
  líder.

## Siguiente paso

1. El líder lanza `.github/workflows/dx11-storage-check.yml` sobre
   `dx11-storage-readonly`, comprueba los cinco pasos de test y el formato, y
   rellena en este documento el commit de código, los run IDs y un extracto del
   log.
2. Si el workflow queda verde, la rama puede integrarse y las filas de storage
   de la Puerta 0 quedan sustituidas por este contrato (nota en
   `dx11-contract-gate0.md`).
3. Después, repetir la cobertura en hardware (Intel HD 4400) antes de afirmar
   paridad. Compute, UAV, storage textures y offsets dinámicos siguen fuera de
   alcance y no deben anunciarse.
