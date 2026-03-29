// pipeline.rs - Graphics Pipe Configuration
pub enum ShaderType {
    Vertex,
    Pixel,
    Compute,
}

pub struct PipelineState {
    pub vs_addr: u64,
    pub ps_addr: u64,
    pub depth_enabled: bool,
}

impl PipelineState {
    pub fn commit(&self) {
        // Push state changes to the Command Buffer
        // This might trigger Forth words like 'bind-pipeline'
    }
}