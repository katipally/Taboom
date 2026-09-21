use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use taboom_proto::Route;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    LinuxKvm,
    MacosHvf,
}

impl Platform {
    pub fn detect() -> Option<Self> {
        if cfg!(target_os = "linux") {
            Some(Self::LinuxKvm)
        } else if cfg!(target_os = "macos") {
            Some(Self::MacosHvf)
        } else {
            None
        }
    }

    pub fn accel_flag(&self) -> &'static str {
        match self {
            Self::LinuxKvm => "kvm",
            Self::MacosHvf => "hvf",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenConfig {
    pub width: u32,
    pub height: u32,
    pub dpr: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QemuConfig {
    pub name: String,
    pub cpus: u32,
    pub ram_mb: u32,
    pub base_image: PathBuf,
    pub overlay_image: PathBuf,
    pub home_image: PathBuf,
    pub cloud_init_iso: Option<PathBuf>,
    pub qmp_socket: PathBuf,
    pub control_socket: PathBuf,
    pub pidfile: PathBuf,
    pub platform: Platform,
    pub display: bool,
    pub route: Route,
    #[serde(skip)]
    pub screen: Option<ScreenConfig>,
}

impl QemuConfig {
    pub fn build_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        args.extend(["-name".into(), self.name.clone()]);
        let is_aarch64 = matches!(self.platform, Platform::MacosHvf)
            || cfg!(target_arch = "aarch64");

        let machine_type = if is_aarch64 {
            format!("virt,accel={},highmem=on", self.platform.accel_flag())
        } else {
            format!("q35,accel={}", self.platform.accel_flag())
        };
        args.extend(["-machine".into(), machine_type]);
        args.extend(["-cpu".into(), "host".into()]);

        if is_aarch64 {
            args.extend([
                "-bios".into(),
                "/opt/homebrew/share/qemu/edk2-aarch64-code.fd".into(),
            ]);
        }
        args.extend(["-smp".into(), self.cpus.to_string()]);
        args.extend(["-m".into(), format!("{}M", self.ram_mb)]);

        args.extend([
            "-drive".into(),
            format!(
                "file={},if=virtio,format=qcow2",
                self.overlay_image.display()
            ),
        ]);

        args.extend([
            "-drive".into(),
            format!(
                "file={},if=virtio,format=qcow2",
                self.home_image.display()
            ),
        ]);

        if let Some(iso) = &self.cloud_init_iso {
            args.extend([
                "-drive".into(),
                format!("file={},if=virtio,media=cdrom", iso.display()),
            ]);
        }

        args.extend([
            "-device".into(),
            "virtio-serial-pci".into(),
            "-chardev".into(),
            format!("socket,id=ctl,path={},server=on,wait=off", self.control_socket.display()),
            "-device".into(),
            "virtserialport,chardev=ctl,name=org.taboom.ctl".into(),
        ]);

        args.extend([
            "-qmp".into(),
            format!("unix:{},server,nowait", self.qmp_socket.display()),
        ]);

        let netdev_str = match &self.route {
            Route::Direct => "user,id=net0,hostfwd=tcp::2222-:22".into(),
            Route::Proxy { address, port, .. } => {
                // restrict=on blocks all guest-initiated traffic except guestfwd.
                // guestfwd routes guest TCP connections on 10.0.2.100:1080 to the
                // host-side proxy, giving the guest exactly one way out (I5).
                format!(
                    "user,id=net0,restrict=on,guestfwd=tcp:10.0.2.100:1080-tcp:{address}:{port}"
                )
            }
        };
        args.extend([
            "-netdev".into(),
            netdev_str,
            "-device".into(),
            "virtio-net-pci,netdev=net0".into(),
        ]);

        args.extend(["-pidfile".into(), self.pidfile.display().to_string()]);

        if !self.display {
            args.extend(["-display".into(), "none".into()]);
        }

        if let Some(screen) = &self.screen {
            let sway_output = format!(
                "WLR_OUTPUT_MODE={}x{}",
                screen.width, screen.height
            );
            args.extend([
                "-fw_cfg".into(),
                format!("name=opt/taboom/screen_width,string={}", screen.width),
                "-fw_cfg".into(),
                format!("name=opt/taboom/screen_height,string={}", screen.height),
                "-fw_cfg".into(),
                format!("name=opt/taboom/screen_dpr,string={}", screen.dpr),
            ]);
            let _ = sway_output;
        }

        args.extend(["-daemonize".into()]);

        args
    }
}
