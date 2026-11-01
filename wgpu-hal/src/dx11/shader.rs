//! D3D11 shader modules and FXC compilation.
//!
//! Naga IR is translated to HLSL with direct sampler bindings and compiled
//! with FXC (`D3DCompile`) for Shader Model 5.0. D3D11 has no descriptor
//! heaps, so the generated HLSL uses flat `b`/`t`/`u`/`s` register spaces.

use alloc::{
    ffi::CString,
    string::{String, ToString as _},
    vec::Vec,
};
use core::fmt;
use std::{
    collections::HashMap,
    hash::{Hash as _, Hasher as _},
};

use parking_lot::Mutex;
use windows::{
    core::PCSTR,
    Win32::Graphics::Direct3D::{Fxc, ID3DBlob, ID3DInclude},
};

use super::PipelineLayout;

/// An entry point declared by a shader module.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderEntryPoint {
    pub name: String,
    pub stage: naga::ShaderStage,
}

/// HLSL source and bytecode produced for one entry point.
#[derive(Clone, Debug)]
pub struct CompiledShader {
    pub hlsl: String,
    pub bytecode: Vec<u8>,
    pub entry_point: String,
    pub stage: naga::ShaderStage,
}

enum ShaderSource {
    Naga(crate::NagaShader),
    Hlsl { source: String },
}

impl fmt::Debug for ShaderSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Naga(_) => f.debug_tuple("Naga").finish_non_exhaustive(),
            Self::Hlsl { source } => f
                .debug_struct("Hlsl")
                .field("source_len", &source.len())
                .finish(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ShaderCacheKey {
    entry_point: String,
    stage: naga::ShaderStage,
    source_hash: u64,
}

/// A D3D11 shader module.
///
/// The module stores the original Naga IR or HLSL source. Compilation is
/// deferred until a pipeline selects an entry point, because HLSL register
/// assignments depend on the pipeline layout.
pub struct ShaderModule {
    label: Option<String>,
    source: ShaderSource,
    entry_points: Vec<ShaderEntryPoint>,
    runtime_checks: wgt::ShaderRuntimeChecks,
    cache: Mutex<HashMap<ShaderCacheKey, Vec<u8>>>,
}

impl fmt::Debug for ShaderModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShaderModule")
            .field("label", &self.label)
            .field("source", &self.source)
            .field("entry_points", &self.entry_points)
            .finish_non_exhaustive()
    }
}

impl ShaderModule {
    pub(crate) fn new(
        desc: &crate::ShaderModuleDescriptor,
        shader: crate::ShaderInput,
    ) -> Result<Self, crate::ShaderError> {
        let cache = Mutex::new(HashMap::new());
        let (source, entry_points) = match shader {
            crate::ShaderInput::Naga(naga) => {
                let entry_points = naga
                    .module
                    .entry_points
                    .iter()
                    .map(|entry| ShaderEntryPoint {
                        name: entry.name.clone(),
                        stage: entry.stage,
                    })
                    .collect();
                (ShaderSource::Naga(naga), entry_points)
            }
            crate::ShaderInput::Hlsl { shader } => (
                ShaderSource::Hlsl {
                    source: shader.to_string(),
                },
                Vec::new(),
            ),
            other => {
                return Err(crate::ShaderError::Compilation(format!(
                    "D3D11 shader modules support Naga IR and HLSL source only, got {other:?}"
                )));
            }
        };

        Ok(Self {
            label: desc.label.map(String::from),
            source,
            entry_points,
            runtime_checks: desc.runtime_checks,
            cache,
        })
    }

    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    pub fn entry_points(&self) -> &[ShaderEntryPoint] {
        &self.entry_points
    }

    /// Translates Naga IR (or accepts HLSL source directly) and compiles the
    /// requested entry point with FXC.
    pub fn compile(
        &self,
        entry_point: &str,
        stage: naga::ShaderStage,
        layout: &PipelineLayout,
        constants: &naga::back::PipelineConstants,
    ) -> Result<CompiledShader, crate::ShaderError> {
        compile_shader(self, entry_point, stage, layout, constants)
    }
}

/// Compiles one entry point of `module` for D3D11.
pub fn compile_shader(
    module: &ShaderModule,
    entry_point: &str,
    stage: naga::ShaderStage,
    layout: &PipelineLayout,
    constants: &naga::back::PipelineConstants,
) -> Result<CompiledShader, crate::ShaderError> {
    if let ShaderSource::Naga(naga) = &module.source {
        if !naga
            .module
            .entry_points
            .iter()
            .any(|entry| entry.name == entry_point && entry.stage == stage)
        {
            return Err(shader_error(
                module.label.as_deref(),
                entry_point,
                stage,
                None,
                "entry point does not exist for the requested stage",
            ));
        }
    }

    let Some(profile) = hlsl_profile(stage) else {
        return Err(shader_error(
            module.label.as_deref(),
            entry_point,
            stage,
            None,
            "D3D11 FXC supports vertex, fragment and compute shaders only",
        ));
    };

    let (hlsl, fxc_entry_point) = match &module.source {
        ShaderSource::Naga(naga) => {
            naga_to_hlsl(naga, module, entry_point, stage, layout, constants)?
        }
        ShaderSource::Hlsl { source } => (source.clone(), entry_point.to_string()),
    };

    let source_hash = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        hlsl.hash(&mut hasher);
        hasher.finish()
    };
    let key = ShaderCacheKey {
        entry_point: fxc_entry_point.clone(),
        stage,
        source_hash,
    };

    if let Some(bytecode) = module.cache.lock().get(&key).cloned() {
        return Ok(CompiledShader {
            hlsl,
            bytecode,
            entry_point: fxc_entry_point,
            stage,
        });
    }

    let bytecode = compile_fxc(
        &hlsl,
        module.label.as_deref(),
        &fxc_entry_point,
        stage,
        &profile,
    )?;
    // `compile_fxc` rejects empty blobs, so a successful shader can never put
    // zero-byte bytecode in the cache.
    debug_assert!(!bytecode.is_empty());
    module.cache.lock().insert(key, bytecode.clone());

    Ok(CompiledShader {
        hlsl,
        bytecode,
        entry_point: fxc_entry_point,
        stage,
    })
}

fn naga_to_hlsl(
    shader: &crate::NagaShader,
    module: &ShaderModule,
    entry_point: &str,
    stage: naga::ShaderStage,
    layout: &PipelineLayout,
    constants: &naga::back::PipelineConstants,
) -> Result<(String, String), crate::ShaderError> {
    use naga::back::hlsl;

    let (module_ir, info) = naga::back::pipeline_constants::process_overrides(
        &shader.module,
        &shader.info,
        Some((stage, entry_point)),
        constants,
    )
    .map_err(|error| {
        shader_error(
            module.label.as_deref(),
            entry_point,
            stage,
            None,
            alloc::format!("pipeline constant or entry point error: {error}"),
        )
    })?;

    let mut options = layout.naga_options().clone();
    options.restrict_indexing = module.runtime_checks.bounds_checks;
    options.force_loop_bounding = module.runtime_checks.force_loop_bounding;
    options.ray_query_initialization_tracking =
        module.runtime_checks.ray_query_initialization_tracking;

    let pipeline_options = hlsl::PipelineOptions {
        entry_point: Some((stage, entry_point.to_string())),
    };

    let mut source = String::new();
    let reflection = {
        let mut writer = hlsl::Writer::new(&mut source, &options, &pipeline_options);
        writer.write(&module_ir, &info, None).map_err(|error| {
            shader_error(
                module.label.as_deref(),
                entry_point,
                stage,
                None,
                alloc::format!("Naga HLSL translation failed: {error}"),
            )
        })?
    };

    let fxc_entry_point = reflection
        .entry_point_names
        .into_iter()
        .next()
        .ok_or_else(|| {
            shader_error(
                module.label.as_deref(),
                entry_point,
                stage,
                None,
                "Naga did not report an HLSL entry point",
            )
        })?
        .map_err(|error| {
            shader_error(
                module.label.as_deref(),
                entry_point,
                stage,
                None,
                alloc::format!("Naga could not resolve entry point: {error}"),
            )
        })?;

    Ok((source, fxc_entry_point))
}

fn hlsl_stage_name(stage: naga::ShaderStage) -> Option<&'static str> {
    match stage {
        naga::ShaderStage::Vertex => Some("vs"),
        naga::ShaderStage::Fragment => Some("ps"),
        naga::ShaderStage::Compute => Some("cs"),
        _ => None,
    }
}

fn hlsl_profile(stage: naga::ShaderStage) -> Option<String> {
    let stage_name = hlsl_stage_name(stage)?;
    Some(alloc::format!(
        "{stage_name}_{}",
        naga::back::hlsl::ShaderModel::V5_0.to_str()
    ))
}

/// Builds a compilation error that identifies the shader module, entry point
/// and stage/profile.
fn shader_error(
    label: Option<&str>,
    entry_point: &str,
    stage: naga::ShaderStage,
    profile: Option<&str>,
    detail: impl fmt::Display,
) -> crate::ShaderError {
    let profile = profile
        .map(String::from)
        .or_else(|| hlsl_profile(stage))
        .unwrap_or_else(|| String::from("unsupported profile"));
    crate::ShaderError::Compilation(alloc::format!(
        "shader '{}' entry point '{}' ({stage:?}, {profile}): {detail}",
        label.unwrap_or("<unnamed>"),
    ))
}

fn compile_fxc(
    source: &str,
    source_name: Option<&str>,
    entry_point: &str,
    stage: naga::ShaderStage,
    profile: &str,
) -> Result<Vec<u8>, crate::ShaderError> {
    let entry_point_c = CString::new(entry_point).map_err(|_| {
        shader_error(
            source_name,
            entry_point,
            stage,
            Some(profile),
            "entry point contains a NUL byte",
        )
    })?;
    let profile_c = CString::new(profile).map_err(|_| {
        shader_error(
            source_name,
            entry_point,
            stage,
            Some(profile),
            "shader profile contains a NUL byte",
        )
    })?;
    let source_name_c = source_name.and_then(|name| CString::new(name).ok());

    let source_name_ptr = source_name_c
        .as_ref()
        .map_or(PCSTR::null(), |name| PCSTR(name.as_ptr().cast()));

    let mut bytecode = None;
    let mut errors = None;
    let result = unsafe {
        Fxc::D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            source_name_ptr,
            None,
            None::<&ID3DInclude>,
            PCSTR(entry_point_c.as_ptr().cast()),
            PCSTR(profile_c.as_ptr().cast()),
            Fxc::D3DCOMPILE_ENABLE_STRICTNESS,
            0,
            &mut bytecode,
            Some(&mut errors as *mut _),
        )
    };

    match result {
        Ok(()) => {
            let blob = bytecode.ok_or_else(|| {
                shader_error(
                    source_name,
                    entry_point,
                    stage,
                    Some(profile),
                    "FXC reported success but returned no bytecode blob",
                )
            })?;
            let length = unsafe { blob.GetBufferSize() };
            if length == 0 {
                return Err(shader_error(
                    source_name,
                    entry_point,
                    stage,
                    Some(profile),
                    "FXC reported success but returned empty bytecode",
                ));
            }

            let pointer = unsafe { blob.GetBufferPointer() }.cast::<u8>();
            if pointer.is_null() {
                return Err(shader_error(
                    source_name,
                    entry_point,
                    stage,
                    Some(profile),
                    "FXC reported success but returned a null bytecode pointer",
                ));
            }

            let bytes = unsafe { core::slice::from_raw_parts(pointer, length) }.to_vec();
            log::debug!(
                "compiled D3D11 shader '{entry_point}' as {profile}: {} bytes",
                bytes.len()
            );
            Ok(bytes)
        }
        Err(error) => {
            let details = errors.as_ref().map(blob_message).unwrap_or_default();
            Err(shader_error(
                source_name,
                entry_point,
                stage,
                Some(profile),
                alloc::format!("FXC failed: {error}{details}"),
            ))
        }
    }
}

fn blob_message(blob: &ID3DBlob) -> String {
    let pointer = unsafe { blob.GetBufferPointer() }.cast::<u8>();
    let length = unsafe { blob.GetBufferSize() };
    if pointer.is_null() || length == 0 {
        return String::new();
    }

    let message = String::from_utf8_lossy(unsafe { core::slice::from_raw_parts(pointer, length) });
    alloc::format!(": {}", message.trim())
}
