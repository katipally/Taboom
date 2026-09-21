use anyhow::{Result, bail};
use taboom_proto::{ImageFormat, Region};
use uuid::Uuid;

pub struct FrameManager {
    current_frame_id: Uuid,
    native_width: u32,
    native_height: u32,
    generation: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ScreenError {
    #[error("stale frame: screen changed since frame {frame_id}, take a new screenshot")]
    StaleFrame { frame_id: Uuid },
    #[error("coordinates ({x}, {y}) out of bounds for {width}x{height} frame")]
    OutOfBounds { x: i32, y: i32, width: u32, height: u32 },
}

impl FrameManager {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            current_frame_id: Uuid::new_v4(),
            native_width: width,
            native_height: height,
            generation: 0,
        }
    }

    pub fn current_frame_id(&self) -> Uuid {
        self.current_frame_id
    }

    pub fn native_size(&self) -> (u32, u32) {
        (self.native_width, self.native_height)
    }

    pub fn update_geometry(&mut self, width: u32, height: u32) {
        if width != self.native_width || height != self.native_height {
            self.native_width = width;
            self.native_height = height;
            self.current_frame_id = Uuid::new_v4();
            self.generation += 1;
        }
    }

    pub fn new_frame(&mut self) -> Uuid {
        self.current_frame_id = Uuid::new_v4();
        self.generation += 1;
        self.current_frame_id
    }

    pub fn validate_coordinates(
        &self,
        x: i32,
        y: i32,
        frame_id: Uuid,
    ) -> Result<(i32, i32), ScreenError> {
        if frame_id != self.current_frame_id {
            return Err(ScreenError::StaleFrame { frame_id });
        }
        if x < 0 || y < 0 || x as u32 >= self.native_width || y as u32 >= self.native_height {
            return Err(ScreenError::OutOfBounds {
                x,
                y,
                width: self.native_width,
                height: self.native_height,
            });
        }
        Ok((x, y))
    }
}

pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

pub trait ScreenCapture: Send + Sync {
    fn capture_frame(&self) -> Result<RawFrame>;
}

pub struct GrimCapture;

impl ScreenCapture for GrimCapture {
    fn capture_frame(&self) -> Result<RawFrame> {
        bail!("grim capture requires running sway compositor")
    }
}

impl GrimCapture {
    pub async fn capture_to_png(&self) -> Result<Vec<u8>> {
        let output = tokio::process::Command::new("grim")
            .args(["-t", "png", "-"])
            .env("WAYLAND_DISPLAY", "wayland-1")
            .env("XDG_RUNTIME_DIR", "/run/user/1000")
            .output()
            .await?;
        if !output.status.success() {
            bail!(
                "grim failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(output.stdout)
    }

    pub async fn capture_region_to_png(&self, region: &Region) -> Result<Vec<u8>> {
        let geometry = format!("{},{} {}x{}", region.x, region.y, region.width, region.height);
        let output = tokio::process::Command::new("grim")
            .args(["-g", &geometry, "-t", "png", "-"])
            .env("WAYLAND_DISPLAY", "wayland-1")
            .env("XDG_RUNTIME_DIR", "/run/user/1000")
            .output()
            .await?;
        if !output.status.success() {
            bail!(
                "grim region capture failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(output.stdout)
    }
}

const JPEG_QUALITY_LADDER: &[u8] = &[95, 85, 70, 50];
const MAX_INLINE_BYTES: usize = 5 * 1024 * 1024;

pub fn apply_byte_budget(
    data: &[u8],
    requested_format: &ImageFormat,
) -> (Vec<u8>, ImageFormat) {
    match requested_format {
        ImageFormat::Png => {
            if data.len() <= MAX_INLINE_BYTES {
                return (data.to_vec(), ImageFormat::Png);
            }
            (data.to_vec(), ImageFormat::Png)
        }
        ImageFormat::Jpeg { quality } => {
            let q = *quality;
            for &ladder_q in JPEG_QUALITY_LADDER {
                if ladder_q <= q {
                    return (data.to_vec(), ImageFormat::Jpeg { quality: ladder_q });
                }
            }
            (data.to_vec(), ImageFormat::Jpeg { quality: 50 })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetCheckResult {
    Unchanged,
    Changed { new_frame_id: Uuid },
}

pub fn check_target(
    _x: i32,
    _y: i32,
    frame_id: Uuid,
    current_frame_id: Uuid,
    _diff_threshold: f64,
) -> TargetCheckResult {
    if frame_id != current_frame_id {
        return TargetCheckResult::Changed {
            new_frame_id: current_frame_id,
        };
    }
    TargetCheckResult::Unchanged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_manager_validate_good_coords() {
        let fm = FrameManager::new(1920, 1080);
        let fid = fm.current_frame_id();
        assert!(fm.validate_coordinates(100, 200, fid).is_ok());
        assert!(fm.validate_coordinates(0, 0, fid).is_ok());
        assert!(fm.validate_coordinates(1919, 1079, fid).is_ok());
    }

    #[test]
    fn frame_manager_reject_oob() {
        let fm = FrameManager::new(1920, 1080);
        let fid = fm.current_frame_id();
        assert!(fm.validate_coordinates(1920, 0, fid).is_err());
        assert!(fm.validate_coordinates(0, 1080, fid).is_err());
        assert!(fm.validate_coordinates(-1, 0, fid).is_err());
    }

    #[test]
    fn frame_manager_reject_stale() {
        let fm = FrameManager::new(1920, 1080);
        let stale = Uuid::new_v4();
        assert!(matches!(
            fm.validate_coordinates(100, 100, stale),
            Err(ScreenError::StaleFrame { .. })
        ));
    }

    #[test]
    fn geometry_change_invalidates_frame() {
        let mut fm = FrameManager::new(1920, 1080);
        let old_fid = fm.current_frame_id();
        fm.update_geometry(2560, 1440);
        assert_ne!(old_fid, fm.current_frame_id());
        assert!(fm.validate_coordinates(100, 100, old_fid).is_err());
    }

    #[test]
    fn target_check_detects_change() {
        let old = Uuid::new_v4();
        let current = Uuid::new_v4();
        assert_eq!(
            check_target(100, 100, old, current, 0.1),
            TargetCheckResult::Changed { new_frame_id: current }
        );
        assert_eq!(
            check_target(100, 100, current, current, 0.1),
            TargetCheckResult::Unchanged
        );
    }
}
