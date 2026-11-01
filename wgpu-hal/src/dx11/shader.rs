//! Shader module stub.
//!
//! The final backend will translate Naga IR to HLSL with `naga::back::hlsl`
//! and compile the result with FXC (`D3DCompile`, enabled by the
//! `Win32_Graphics_Direct3D_Fxc` feature). Shader compilation is intentionally
//! not wired into [`crate::Device::create_shader_module`] yet, so this module
//! only provides the resource marker required by [`super::Api`].

/// A compiled HLSL shader module.
///
/// This will eventually own an `ID3DBlob` produced by FXC, plus the
/// `ID3D11VertexShader` / `ID3D11PixelShader` / `ID3D11ComputeShader` objects
/// created for each entry point. For now it is an empty marker so that
/// `Api::ShaderModule` satisfies the `DynShaderModule` bound.
#[derive(Debug)]
pub struct ShaderModule;
