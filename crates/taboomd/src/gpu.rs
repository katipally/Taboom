use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuTier {
    VfioPassthrough,
    VirtioHostGl,
    SoftwareRendering,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuCapabilities {
    pub tier: GpuTier,
    pub webgl_available: bool,
    pub renderer_string: String,
    pub extension_count: u32,
    pub canvas_hash_stable: bool,
}

impl GpuTier {
    pub fn capabilities(&self) -> GpuCapabilities {
        match self {
            GpuTier::VfioPassthrough => GpuCapabilities {
                tier: self.clone(),
                webgl_available: true,
                renderer_string: "ANGLE (passthrough GPU)".into(),
                extension_count: 45,
                canvas_hash_stable: true,
            },
            GpuTier::VirtioHostGl => GpuCapabilities {
                tier: self.clone(),
                webgl_available: true,
                renderer_string: "ANGLE (virtio-gpu, virglrenderer)".into(),
                extension_count: 30,
                canvas_hash_stable: true,
            },
            GpuTier::SoftwareRendering => GpuCapabilities {
                tier: self.clone(),
                webgl_available: true,
                renderer_string: "ANGLE (llvmpipe)".into(),
                extension_count: 20,
                canvas_hash_stable: false,
            },
        }
    }
}

pub fn select_gpu_tier(has_vfio: bool, has_virgl: bool) -> GpuTier {
    if has_vfio {
        GpuTier::VfioPassthrough
    } else if has_virgl {
        GpuTier::VirtioHostGl
    } else {
        GpuTier::SoftwareRendering
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_selection() {
        assert_eq!(select_gpu_tier(true, true), GpuTier::VfioPassthrough);
        assert_eq!(select_gpu_tier(false, true), GpuTier::VirtioHostGl);
        assert_eq!(select_gpu_tier(false, false), GpuTier::SoftwareRendering);
    }

    #[test]
    fn capabilities_have_webgl() {
        for tier in [GpuTier::VfioPassthrough, GpuTier::VirtioHostGl, GpuTier::SoftwareRendering] {
            assert!(tier.capabilities().webgl_available);
        }
    }
}
