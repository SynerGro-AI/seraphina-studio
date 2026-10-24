//! Camera capture for the Camera output source: V4L2 on Linux, Media
//! Foundation on Windows. Frames come back as RGBA `Raster`s, the same
//! type the canvas hands to ffmpeg. Other systems get a clear refusal.

use crate::output::FrameSlot;
use obs_rust_bitstream::ui_canvas::Raster;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// A camera the system reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// What the driver calls it (e.g. "HD Pro Webcam C920").
    pub name: String,
    /// `/dev/videoN` on Linux, the Media Foundation symbolic link on Windows.
    pub path: String,
}

/// Why cameras can't be listed or a camera can't be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CameraError {
    /// A camera is there but the system won't let the app use it: Linux
    /// device permissions (the video group) or the Windows camera privacy
    /// setting. Settings ▸ Output offers Approve (see [`approve`]).
    Blocked(String),
    Failed(String),
}

impl CameraError {
    fn prefixed(self, p: &str) -> Self {
        match self {
            Self::Blocked(m) => Self::Blocked(format!("{p}: {m}")),
            Self::Failed(m) => Self::Failed(format!("{p}: {m}")),
        }
    }
}

impl std::fmt::Display for CameraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

impl From<String> for CameraError {
    fn from(m: String) -> Self {
        Self::Failed(m)
    }
}

impl From<&str> for CameraError {
    fn from(m: &str) -> Self {
        Self::Failed(m.into())
    }
}

impl From<CameraError> for String {
    fn from(e: CameraError) -> Self {
        e.to_string()
    }
}

/// What Approve does for blocked camera access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approve {
    /// Open this Settings page (Windows camera privacy).
    OpenSettings(&'static str),
    /// Show the command that fixes it and what to do after.
    ShowFix {
        command: &'static str,
        then: &'static str,
    },
}

/// Windows Settings ▸ Privacy & security ▸ Camera.
pub const WINDOWS_CAMERA_PRIVACY: &str = "ms-settings:privacy-webcam";

/// Approve on this OS; `None` where cameras aren't supported yet (macOS).
pub fn approve() -> Option<Approve> {
    if cfg!(windows) {
        Some(Approve::OpenSettings(WINDOWS_CAMERA_PRIVACY))
    } else if cfg!(target_os = "linux") {
        Some(Approve::ShowFix {
            command: "sudo usermod -aG video $USER",
            then: "then log out and back in; the app checks again by itself",
        })
    } else {
        None
    }
}

/// Opens a Settings URI the way Explorer would (`start` hands it to its
/// registered handler).
pub fn settings_command(uri: &str) -> std::process::Command {
    let mut c = std::process::Command::new("cmd");
    c.args(["/C", "start", "", uri]);
    c
}

/// A Media Foundation activation failure: E_ACCESSDENIED is the camera
/// privacy setting refusing desktop apps.
#[cfg(any(windows, test))]
fn activate_error(name: &str, hresult: i32, msg: &str) -> CameraError {
    const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
    if hresult == E_ACCESSDENIED {
        CameraError::Blocked(format!(
            "Windows camera privacy settings block {name} (Let desktop apps access your camera is off)"
        ))
    } else {
        CameraError::Failed(format!("{name}: activate: {msg}"))
    }
}

/// Pick the device named by `want` (its path, or its name ignoring case);
/// empty means the first one.
pub fn choose<'a>(devices: &'a [Device], want: &str) -> Result<&'a Device, String> {
    let want = want.trim();
    if want.is_empty() {
        return devices.first().ok_or_else(|| {
            "no camera found. Plug one in, then press Refresh in Settings ▸ Output.".into()
        });
    }
    devices
        .iter()
        .find(|d| d.path == want || d.name.eq_ignore_ascii_case(want))
        .ok_or_else(|| {
            let names: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
            format!(
                "camera '{want}' not found (available: {})",
                if names.is_empty() {
                    "none".to_string()
                } else {
                    names.join(", ")
                }
            )
        })
}

/// What a camera scan found. Nothing is dropped: cameras the user may
/// not open and nodes that failed are named alongside the usable ones.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub cameras: Vec<Device>,
    /// Cameras that are there but blocked (see [`CameraError::Blocked`]).
    pub blocked: Vec<Unusable>,
    /// Nodes that couldn't be opened for another reason.
    pub failed: Vec<Unusable>,
}

/// A device node that is listed but can't be used, and why (the message
/// names the node).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    pub path: String,
    pub why: String,
}

impl Listing {
    /// Sorts each node's probe: a camera, not a camera (skipped: a
    /// metadata or output node), or why it couldn't be opened.
    #[cfg(any(target_os = "linux", test))]
    pub fn from_probes(
        probes: impl IntoIterator<Item = (String, Result<Option<Device>, CameraError>)>,
    ) -> Self {
        let mut l = Listing::default();
        for (path, p) in probes {
            match p {
                Ok(Some(d)) => l.cameras.push(d),
                Ok(None) => {}
                Err(CameraError::Blocked(why)) => l.blocked.push(Unusable { path, why }),
                Err(CameraError::Failed(why)) => l.failed.push(Unusable { path, why }),
            }
        }
        l
    }

    /// Every blocked camera's reason, one per line.
    pub fn blocked_why(&self) -> Option<String> {
        let w: Vec<&str> = self.blocked.iter().map(|u| u.why.as_str()).collect();
        (!w.is_empty()).then(|| w.join("\n"))
    }

    /// The camera `want` names (see [`choose`]). With none to pick,
    /// blocked cameras make it `Blocked`, so Approve is offered.
    pub fn pick(&self, want: &str) -> Result<&Device, CameraError> {
        choose(&self.cameras, want).map_err(|e| match self.blocked_why() {
            None => CameraError::Failed(e),
            // a camera is there, just blocked: not "no camera found"
            Some(why) if self.cameras.is_empty() => CameraError::Blocked(why.replace('\n', "; ")),
            Some(why) => CameraError::Blocked(format!("{e}; {}", why.replace('\n', "; "))),
        })
    }
}

/// A camera being read on its own thread. The newest frame sits in
/// `frames`, where recording, streaming and the Settings preview read it.
pub struct Feed {
    pub device: Device,
    pub frames: FrameSlot,
    /// (width, height) the camera actually delivers.
    pub size: (u32, u32),
    read: Arc<AtomicU64>,
    error: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Feed {
    /// Open the camera named by `want` (see [`choose`]) at about
    /// `width`×`height` and start reading. Fails if it can't be opened.
    pub fn start(want: &str, width: u32, height: u32) -> Result<Feed, CameraError> {
        let device = list()?.pick(want)?.clone();
        let dev = device.clone();
        Self::spawn(device, move || Camera::open(&dev, width, height))
    }

    /// Run `open` on a new thread (Media Foundation objects stay on the
    /// thread that made them) and keep reading frames until dropped.
    fn spawn<C: Read>(
        device: Device,
        open: impl FnOnce() -> Result<C, CameraError> + Send + 'static,
    ) -> Result<Feed, CameraError> {
        let frames: FrameSlot = Arc::new(Mutex::new(None));
        let read = Arc::new(AtomicU64::new(0));
        let error = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (f, n, e, s) = (frames.clone(), read.clone(), error.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            let mut cam = match open() {
                Ok(c) => c,
                Err(err) => {
                    let _ = tx.send(Err(err));
                    return;
                }
            };
            let first = cam.next_frame(Duration::from_secs(5));
            let _ = tx.send(
                first
                    .as_ref()
                    .map(|r| (r.width, r.height))
                    .map_err(|e| CameraError::Failed(e.clone())),
            );
            let Ok(first) = first else { return };
            *f.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(first));
            n.fetch_add(1, Ordering::Relaxed);
            while !s.load(Ordering::Relaxed) {
                match cam.next_frame(Duration::from_secs(2)) {
                    Ok(r) => {
                        *f.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(r));
                        n.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(err) => {
                        *e.lock().unwrap_or_else(|p| p.into_inner()) = Some(err);
                        return;
                    }
                }
            }
        });
        let size = match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(r) => r.map_err(|e| e.prefixed(&format!("camera '{}'", device.name)))?,
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                return Err(format!("camera '{}' did not start in 15 s", device.name).into());
            }
        };
        Ok(Feed {
            device,
            frames,
            size,
            read,
            error,
            stop,
            thread: Some(thread),
        })
    }

    /// Why reading stopped, once it has.
    pub fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|e| e.clone())
    }

    /// Frames read so far.
    pub fn frames_read(&self) -> u64 {
        self.read.load(Ordering::Relaxed)
    }

    pub fn latest(&self) -> Option<Arc<Raster>> {
        self.frames.lock().ok().and_then(|f| f.clone())
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // A V4L2 read returns within 2 s; a stalled Media Foundation read
        // can block longer, so after 3 s the thread is left to exit on its
        // own at its next frame rather than freezing the UI.
        if let Some(t) = self.thread.take() {
            let until = std::time::Instant::now() + Duration::from_secs(3);
            while !t.is_finished() && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

/// Something that produces camera frames: [`Camera`], or recorded frames
/// in tests.
pub trait Read {
    fn next_frame(&mut self, timeout: Duration) -> Result<Raster, String>;
}

impl Read for Camera {
    fn next_frame(&mut self, timeout: Duration) -> Result<Raster, String> {
        self.read(timeout)
    }
}

#[cfg(test)]
impl Feed {
    /// A feed whose frames come from `cam` instead of a device.
    pub fn from_reader<C: Read + Send + 'static>(name: &str, cam: C) -> Result<Feed, CameraError> {
        let device = Device {
            name: name.into(),
            path: name.into(),
        };
        Self::spawn(device, move || Ok(cam))
    }
}

/// BT.601 limited-range YUV to RGB.
#[cfg(any(target_os = "linux", test))]
fn yuv_px(y: u8, u: u8, v: u8, out: &mut [u8]) {
    let c = 298 * (y as i32 - 16);
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    out[0] = ((c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
    out[1] = ((c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
    out[2] = ((c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
    out[3] = 255;
}

#[cfg(any(target_os = "linux", test))]
/// Packed YUYV 4:2:2 (Y0 U Y1 V), `stride` bytes per row, to RGBA.
pub fn yuyv_to_rgba(src: &[u8], width: u32, height: u32, stride: usize) -> Result<Raster, String> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || w % 2 != 0 || stride < w * 2 {
        return Err(format!(
            "bad YUYV geometry {width}x{height}, stride {stride}"
        ));
    }
    if src.len() < stride * (h - 1) + w * 2 {
        return Err(format!(
            "YUYV frame too short: {} bytes for {width}x{height}",
            src.len()
        ));
    }
    let mut data = vec![0u8; w * h * 4];
    for (y, row) in data.chunks_exact_mut(w * 4).enumerate() {
        let s = &src[y * stride..y * stride + w * 2];
        for (px, q) in row
            .as_chunks_mut::<8>()
            .0
            .iter_mut()
            .zip(s.as_chunks::<4>().0)
        {
            yuv_px(q[0], q[1], q[3], &mut px[..4]);
            yuv_px(q[2], q[1], q[3], &mut px[4..]);
        }
    }
    Ok(Raster {
        width,
        height,
        data,
    })
}

/// How a camera packs a frame: what V4L2 captures in, and what the
/// recorded test frames in tests/fixtures/camera use.
#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug)]
pub enum Packing {
    Yuyv {
        width: u32,
        height: u32,
        stride: usize,
    },
    Mjpeg,
}

#[cfg(any(target_os = "linux", test))]
/// One captured frame to RGBA.
pub fn decode(packing: Packing, bytes: &[u8]) -> Result<Raster, String> {
    match packing {
        Packing::Yuyv {
            width,
            height,
            stride,
        } => yuyv_to_rgba(bytes, width, height, stride),
        Packing::Mjpeg => image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
            .map(|i| {
                let i = i.to_rgba8();
                Raster {
                    width: i.width(),
                    height: i.height(),
                    data: i.into_raw(),
                }
            })
            .map_err(|e| format!("bad MJPEG frame: {e}")),
    }
}

#[cfg(any(windows, test))]
/// 32-bit BGRX rows to RGBA. `pitch` is the byte step from one image row
/// to the next as Media Foundation reports it; it is negative for bottom-up
/// images, whose bottom row comes first in `src`.
pub fn bgrx_to_rgba(src: &[u8], width: u32, height: u32, pitch: isize) -> Result<Raster, String> {
    let (w, h) = (width as usize, height as usize);
    let step = pitch.unsigned_abs();
    if w == 0 || h == 0 || step < w * 4 {
        return Err(format!(
            "bad RGB32 geometry {width}x{height}, pitch {pitch}"
        ));
    }
    if src.len() < step * (h - 1) + w * 4 {
        return Err(format!(
            "RGB32 frame too short: {} bytes for {width}x{height}",
            src.len()
        ));
    }
    let mut data = vec![0u8; w * h * 4];
    for (y, row) in data.chunks_exact_mut(w * 4).enumerate() {
        let at = if pitch < 0 {
            (h - 1 - y) * step
        } else {
            y * step
        };
        let px = src[at..at + w * 4].as_chunks::<4>().0;
        for (d, s) in row.as_chunks_mut::<4>().0.iter_mut().zip(px) {
            *d = [s[2], s[1], s[0], 255];
        }
    }
    Ok(Raster {
        width,
        height,
        data,
    })
}

#[cfg(windows)]
pub use mf::{list, Camera};
#[cfg(target_os = "linux")]
pub use v4l2::{list, Camera};

#[cfg(not(any(target_os = "linux", windows)))]
const UNSUPPORTED: &str =
    "Camera capture isn't supported on this OS yet (Windows and Linux only). Pick Canvas or Screen in Settings ▸ Output.";

#[cfg(not(any(target_os = "linux", windows)))]
pub fn list() -> Result<Listing, CameraError> {
    Err(UNSUPPORTED.into())
}

/// No camera API on this OS: opening always fails with the reason.
#[cfg(not(any(target_os = "linux", windows)))]
pub enum Camera {}

#[cfg(not(any(target_os = "linux", windows)))]
impl Camera {
    pub fn open(_dev: &Device, _width: u32, _height: u32) -> Result<Camera, CameraError> {
        Err(UNSUPPORTED.into())
    }
    pub fn read(&mut self, _timeout: Duration) -> Result<Raster, String> {
        match *self {}
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod v4l2 {
    //! Video4Linux2 streaming capture through mmap'd buffers. Struct
    //! layouts follow `<linux/videodev2.h>`; ioctl numbers are derived
    //! from the struct sizes, like the C macros.

    use super::{CameraError, Device, Listing, Raster};
    use std::ffi::CString;
    use std::os::raw::{c_int, c_ulong, c_void};
    use std::time::Duration;

    pub(super) const CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
    pub(super) const CAP_STREAMING: u32 = 0x0400_0000;
    pub(super) const CAP_DEVICE_CAPS: u32 = 0x8000_0000;
    const BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
    const MEMORY_MMAP: u32 = 1;
    const FIELD_NONE: u32 = 1;
    pub(super) const fn fourcc(s: &[u8; 4]) -> u32 {
        s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16 | (s[3] as u32) << 24
    }
    pub(super) const PIX_YUYV: u32 = fourcc(b"YUYV");
    pub(super) const PIX_MJPEG: u32 = fourcc(b"MJPG");

    #[repr(C)]
    pub(super) struct Capability {
        pub driver: [u8; 16],
        pub card: [u8; 32],
        pub bus_info: [u8; 32],
        pub version: u32,
        pub capabilities: u32,
        pub device_caps: u32,
        pub reserved: [u32; 3],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub(super) struct PixFormat {
        pub width: u32,
        pub height: u32,
        pub pixelformat: u32,
        pub field: u32,
        pub bytesperline: u32,
        pub sizeimage: u32,
        pub colorspace: u32,
        pub priv_: u32,
        pub flags: u32,
        pub ycbcr_enc: u32,
        pub quantization: u32,
        pub xfer_func: u32,
    }

    /// `v4l2_format.fmt`: 200 bytes, pointer-aligned (the kernel union
    /// holds `v4l2_window`, which has pointers).
    #[repr(C)]
    pub(super) union FormatUnion {
        pub pix: PixFormat,
        pub raw: [u8; 200],
        _align: *mut c_void,
    }

    #[repr(C)]
    pub(super) struct Format {
        pub type_: u32,
        pub fmt: FormatUnion,
    }

    #[repr(C)]
    pub(super) struct RequestBuffers {
        pub count: u32,
        pub type_: u32,
        pub memory: u32,
        pub capabilities: u32,
        pub flags: u8,
        pub reserved: [u8; 3],
    }

    #[repr(C)]
    pub(super) struct Timecode {
        pub type_: u32,
        pub flags: u32,
        pub frames: u8,
        pub seconds: u8,
        pub minutes: u8,
        pub hours: u8,
        pub userbits: [u8; 4],
    }

    /// `v4l2_buffer.m`: offset / userptr / planes / fd.
    #[repr(C)]
    pub(super) union BufferM {
        pub offset: u32,
        pub userptr: c_ulong,
        _planes: *mut c_void,
        pub fd: i32,
    }

    #[repr(C)]
    pub(super) struct Buffer {
        pub index: u32,
        pub type_: u32,
        pub bytesused: u32,
        pub flags: u32,
        pub field: u32,
        pub timestamp: libc::timeval,
        pub timecode: Timecode,
        pub sequence: u32,
        pub memory: u32,
        pub m: BufferM,
        pub length: u32,
        pub reserved2: u32,
        pub request_fd: i32,
    }

    const fn ioc(dir: u32, nr: u32, size: usize) -> c_ulong {
        ((dir << 30) | ((size as u32) << 16) | ((b'V' as u32) << 8) | nr) as c_ulong
    }
    const R: u32 = 2;
    const W: u32 = 1;
    const RW: u32 = 3;
    pub(super) const QUERYCAP: c_ulong = ioc(R, 0, size_of::<Capability>());
    pub(super) const S_FMT: c_ulong = ioc(RW, 5, size_of::<Format>());
    pub(super) const REQBUFS: c_ulong = ioc(RW, 8, size_of::<RequestBuffers>());
    pub(super) const QUERYBUF: c_ulong = ioc(RW, 9, size_of::<Buffer>());
    pub(super) const QBUF: c_ulong = ioc(RW, 15, size_of::<Buffer>());
    pub(super) const DQBUF: c_ulong = ioc(RW, 17, size_of::<Buffer>());
    pub(super) const STREAMON: c_ulong = ioc(W, 18, size_of::<c_int>());
    pub(super) const STREAMOFF: c_ulong = ioc(W, 19, size_of::<c_int>());

    fn zeroed<T>() -> T {
        // SAFETY: only used for the plain-old-data kernel structs above,
        // for which all-zero bytes are a valid value.
        unsafe { std::mem::zeroed() }
    }

    /// `ioctl`, retried on EINTR.
    fn xioctl<T>(fd: c_int, req: c_ulong, arg: &mut T) -> std::io::Result<()> {
        loop {
            // SAFETY: `arg` is the struct this request number was built from.
            let r = unsafe { libc::ioctl(fd, req as _, arg as *mut T as *mut c_void) };
            if r != -1 {
                return Ok(());
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }

    struct Fd(c_int);
    impl Drop for Fd {
        fn drop(&mut self) {
            // SAFETY: we own the descriptor.
            unsafe { libc::close(self.0) };
        }
    }

    fn open_fd(path: &str) -> std::io::Result<Fd> {
        let c = CString::new(path).map_err(|_| std::io::ErrorKind::InvalidInput)?;
        // SAFETY: valid NUL-terminated path.
        let fd = unsafe {
            libc::open(
                c.as_ptr(),
                libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Fd(fd))
    }

    fn c_str(b: &[u8]) -> String {
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        String::from_utf8_lossy(&b[..end]).trim().to_string()
    }

    /// The card name if `fd` is a V4L2 device that can stream video capture.
    fn query(fd: &Fd, path: &str) -> Result<String, String> {
        let mut cap: Capability = zeroed();
        xioctl(fd.0, QUERYCAP, &mut cap)
            .map_err(|e| format!("{path} is not a V4L2 device ({e})"))?;
        let caps = if cap.capabilities & CAP_DEVICE_CAPS != 0 {
            cap.device_caps
        } else {
            cap.capabilities
        };
        if caps & CAP_VIDEO_CAPTURE == 0 || caps & CAP_STREAMING == 0 {
            return Err(format!("{path} can't stream video capture"));
        }
        Ok(c_str(&cap.card))
    }

    /// Every `/dev/videoN` that can stream video capture (metadata and
    /// output nodes are skipped).
    pub fn list() -> Result<Listing, CameraError> {
        list_in(std::path::Path::new("/dev"))
    }

    /// Why `path` didn't open: a permission error means the camera is
    /// there but this user may not use it.
    pub(super) fn open_error(path: &str, e: std::io::Error) -> CameraError {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            CameraError::Blocked(format!(
                "Permission denied on {path}: your user isn't allowed to use cameras (not in the video group)"
            ))
        } else {
            CameraError::Failed(format!("can't open {path}: {e}"))
        }
    }

    /// One node: a camera (`Some`), something else (`None`), or why it
    /// can't be opened.
    pub(crate) fn probe(path: &str) -> Result<Option<Device>, CameraError> {
        let fd = open_fd(path).map_err(|e| open_error(path, e))?;
        Ok(query(&fd, path).ok().map(|name| Device {
            name,
            path: path.into(),
        }))
    }

    /// [`list`] over the `videoN` nodes in `dir`, every node accounted for.
    pub(super) fn list_in(dir: &std::path::Path) -> Result<Listing, CameraError> {
        let mut nodes: Vec<(u32, String)> = std::fs::read_dir(dir)
            .map_err(|e| format!("can't read {}: {e}", dir.display()))?
            .filter_map(|e| {
                let name = e.ok()?.file_name().into_string().ok()?;
                let n = name.strip_prefix("video")?.parse().ok()?;
                Some((n, dir.join(&name).to_string_lossy().into_owned()))
            })
            .collect();
        nodes.sort();
        Ok(Listing::from_probes(
            nodes.into_iter().map(|(_, p)| (p.clone(), probe(&p))),
        ))
    }

    struct Map {
        ptr: *mut c_void,
        len: usize,
    }

    /// An open, streaming camera.
    pub struct Camera {
        fd: Fd,
        maps: Vec<Map>,
        width: u32,
        height: u32,
        stride: usize,
        pixfmt: u32,
        path: String,
    }

    impl Camera {
        /// Open `dev` asking for `width`×`height` YUYV (MJPEG if the camera
        /// has no YUYV); the driver picks the nearest size it supports.
        pub fn open(dev: &Device, width: u32, height: u32) -> Result<Camera, CameraError> {
            Self::open_path(&dev.path, width, height)
        }

        pub(super) fn open_path(
            path: &str,
            width: u32,
            height: u32,
        ) -> Result<Camera, CameraError> {
            let fd = open_fd(path).map_err(|e| open_error(path, e))?;
            query(&fd, path)?;
            let mut f: Format = zeroed();
            for want in [PIX_YUYV, PIX_MJPEG] {
                f = zeroed();
                f.type_ = BUF_TYPE_VIDEO_CAPTURE;
                f.fmt.pix = PixFormat {
                    width,
                    height,
                    pixelformat: want,
                    field: FIELD_NONE,
                    ..zeroed()
                };
                xioctl(fd.0, S_FMT, &mut f).map_err(|e| format!("{path}: VIDIOC_S_FMT: {e}"))?;
                // SAFETY: S_FMT on a capture buffer type fills `pix`.
                if unsafe { f.fmt.pix.pixelformat } == want {
                    break;
                }
            }
            // SAFETY: as above.
            let pix = unsafe { f.fmt.pix };
            if pix.pixelformat != PIX_YUYV && pix.pixelformat != PIX_MJPEG {
                let cc = pix.pixelformat.to_le_bytes();
                return Err(format!(
                    "{path} offers neither YUYV nor MJPEG (it chose {})",
                    String::from_utf8_lossy(&cc)
                )
                .into());
            }
            let mut req = RequestBuffers {
                count: 4,
                type_: BUF_TYPE_VIDEO_CAPTURE,
                memory: MEMORY_MMAP,
                ..zeroed()
            };
            xioctl(fd.0, REQBUFS, &mut req).map_err(|e| format!("{path}: VIDIOC_REQBUFS: {e}"))?;
            if req.count == 0 {
                return Err(format!("{path}: the driver gave no capture buffers").into());
            }
            let mut cam = Camera {
                fd,
                maps: Vec::new(),
                width: pix.width,
                height: pix.height,
                stride: (pix.bytesperline as usize).max(pix.width as usize * 2),
                pixfmt: pix.pixelformat,
                path: path.to_string(),
            };
            for i in 0..req.count {
                let mut b = cam.buffer(i);
                xioctl(cam.fd.0, QUERYBUF, &mut b)
                    .map_err(|e| format!("{path}: VIDIOC_QUERYBUF: {e}"))?;
                // SAFETY: the offset and length come from QUERYBUF.
                let ptr = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        b.length as usize,
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_SHARED,
                        cam.fd.0,
                        b.m.offset as libc::off_t,
                    )
                };
                if ptr == libc::MAP_FAILED {
                    return Err(format!("{path}: mmap: {}", std::io::Error::last_os_error()).into());
                }
                cam.maps.push(Map {
                    ptr,
                    len: b.length as usize,
                });
                xioctl(cam.fd.0, QBUF, &mut b).map_err(|e| format!("{path}: VIDIOC_QBUF: {e}"))?;
            }
            let mut t = BUF_TYPE_VIDEO_CAPTURE as c_int;
            xioctl(cam.fd.0, STREAMON, &mut t)
                .map_err(|e| format!("{path}: VIDIOC_STREAMON: {e}"))?;
            Ok(cam)
        }

        fn buffer(&self, index: u32) -> Buffer {
            let mut b: Buffer = zeroed();
            b.index = index;
            b.type_ = BUF_TYPE_VIDEO_CAPTURE;
            b.memory = MEMORY_MMAP;
            b
        }

        /// The next frame, waiting at most `timeout` for it.
        pub fn read(&mut self, timeout: Duration) -> Result<Raster, String> {
            let path = self.path.clone();
            let mut p = libc::pollfd {
                fd: self.fd.0,
                events: libc::POLLIN,
                revents: 0,
            };
            let ms = timeout.as_millis().min(i32::MAX as u128) as c_int;
            // SAFETY: one valid pollfd.
            let n = unsafe { libc::poll(&mut p, 1, ms) };
            if n < 0 {
                return Err(format!("{path}: poll: {}", std::io::Error::last_os_error()));
            }
            if n == 0 {
                return Err(format!("{path}: no frame within {} ms", ms));
            }
            if p.revents & (libc::POLLERR | libc::POLLHUP) != 0 {
                return Err(format!("{path}: the camera went away"));
            }
            let mut b = self.buffer(0);
            xioctl(self.fd.0, DQBUF, &mut b).map_err(|e| format!("{path}: VIDIOC_DQBUF: {e}"))?;
            let m = self
                .maps
                .get(b.index as usize)
                .ok_or_else(|| format!("{path}: driver returned buffer {}", b.index))?;
            let used = (b.bytesused as usize).min(m.len);
            // SAFETY: the buffer is dequeued, so the driver isn't writing it.
            let bytes = unsafe { std::slice::from_raw_parts(m.ptr as *const u8, used) };
            let packing = if self.pixfmt == PIX_YUYV {
                super::Packing::Yuyv {
                    width: self.width,
                    height: self.height,
                    stride: self.stride,
                }
            } else {
                super::Packing::Mjpeg
            };
            let frame = super::decode(packing, bytes).map_err(|e| format!("{path}: {e}"));
            xioctl(self.fd.0, QBUF, &mut b).map_err(|e| format!("{path}: VIDIOC_QBUF: {e}"))?;
            frame
        }
    }

    impl Drop for Camera {
        fn drop(&mut self) {
            let mut t = BUF_TYPE_VIDEO_CAPTURE as c_int;
            let _ = xioctl(self.fd.0, STREAMOFF, &mut t);
            for m in &self.maps {
                // SAFETY: mapped in `open_path`.
                unsafe { libc::munmap(m.ptr, m.len) };
            }
        }
    }
}

#[cfg(windows)]
mod mf {
    //! Media Foundation source reader with the built-in video processor
    //! turned on, so every camera (YUY2, NV12, MJPEG) arrives as RGB32.
    //! COM is initialised (multithreaded) on the thread that uses it.

    use super::{CameraError, Device, Listing, Raster};
    use std::time::Duration;
    use windows::core::{Interface, GUID, PWSTR};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
    };

    /// COM + Media Foundation for the current thread, undone on drop.
    struct Mf;
    impl Mf {
        fn start() -> Result<Mf, String> {
            // SAFETY: paired with CoUninitialize in Drop.
            unsafe {
                CoInitializeEx(None, COINIT_MULTITHREADED)
                    .ok()
                    .map_err(|e| format!("COM init: {e}"))?;
                if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                    CoUninitialize();
                    return Err(format!("Media Foundation start: {e}"));
                }
            }
            Ok(Mf)
        }
    }
    impl Drop for Mf {
        fn drop(&mut self) {
            // SAFETY: started in `Mf::start` on this thread.
            unsafe {
                let _ = MFShutdown();
                CoUninitialize();
            }
        }
    }

    fn string(a: &IMFActivate, key: &GUID) -> Result<String, String> {
        let mut p = PWSTR::null();
        let mut n = 0u32;
        // SAFETY: MF allocates the string; we free it with CoTaskMemFree.
        unsafe {
            a.GetAllocatedString(key, &mut p, &mut n)
                .map_err(|e| e.to_string())?;
            let s = p.to_string().map_err(|e| e.to_string());
            CoTaskMemFree(Some(p.0 as *const _));
            s
        }
    }

    fn devices() -> Result<Vec<(Device, IMFActivate)>, String> {
        // SAFETY: standard MF enumeration; the array is freed below.
        unsafe {
            let mut attr = None;
            MFCreateAttributes(&mut attr, 1).map_err(|e| format!("MFCreateAttributes: {e}"))?;
            let attr = attr.ok_or("MFCreateAttributes returned nothing")?;
            attr.SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(|e| format!("SetGUID: {e}"))?;
            let mut arr: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFEnumDeviceSources(&attr, &mut arr, &mut count)
                .map_err(|e| format!("MFEnumDeviceSources: {e}"))?;
            let mut out = Vec::new();
            for i in 0..count as usize {
                if let Some(a) = std::ptr::read(arr.add(i)) {
                    let name = string(&a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)?;
                    let path =
                        string(&a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK)?;
                    out.push((Device { name, path }, a));
                }
            }
            if !arr.is_null() {
                CoTaskMemFree(Some(arr as *const _));
            }
            Ok(out)
        }
    }

    /// Video capture devices. Runs on its own thread so the caller's COM
    /// apartment (the UI thread is single-threaded) doesn't matter.
    /// Windows refuses blocked access only at activation (see
    /// `activate_error`), so the list has no blocked entries.
    pub fn list() -> Result<Listing, CameraError> {
        let cameras = std::thread::spawn(|| -> Result<Vec<Device>, String> {
            let _mf = Mf::start()?;
            Ok(devices()?.into_iter().map(|(d, _)| d).collect())
        })
        .join()
        .map_err(|_| "camera enumeration panicked".to_string())?
        .map_err(CameraError::Failed)?;
        Ok(Listing {
            cameras,
            ..Listing::default()
        })
    }

    /// An open camera. Use it on the thread that opened it.
    pub struct Camera {
        reader: IMFSourceReader,
        width: u32,
        height: u32,
        /// Bytes from one image row to the next in a plain (non-2D)
        /// buffer; negative for bottom-up RGB, MF's default.
        stride: isize,
        // dropped last: shuts Media Foundation down after the reader
        _mf: Mf,
    }

    const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

    impl Camera {
        /// Open `dev` (matched by symbolic link) asking for RGB32 at
        /// `width`×`height`; the video processor scales if needed.
        pub fn open(dev: &Device, width: u32, height: u32) -> Result<Camera, CameraError> {
            let mf = Mf::start()?;
            let (_, act) = devices()?
                .into_iter()
                .find(|(d, _)| d.path == dev.path)
                .ok_or_else(|| format!("camera '{}' is no longer connected", dev.name))?;
            // SAFETY: standard MF source-reader setup.
            unsafe {
                let source: IMFMediaSource = act
                    .ActivateObject()
                    .map_err(|e| super::activate_error(&dev.name, e.code().0, &e.to_string()))?;
                let mut attr = None;
                MFCreateAttributes(&mut attr, 1).map_err(|e| format!("MFCreateAttributes: {e}"))?;
                let attr = attr.ok_or("MFCreateAttributes returned nothing")?;
                attr.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                    .map_err(|e| format!("SetUINT32: {e}"))?;
                let reader = MFCreateSourceReaderFromMediaSource(&source, &attr)
                    .map_err(|e| format!("{}: source reader: {e}", dev.name))?;
                let mt = MFCreateMediaType().map_err(|e| format!("MFCreateMediaType: {e}"))?;
                mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                    .map_err(|e| e.to_string())?;
                mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                    .map_err(|e| e.to_string())?;
                mt.SetUINT64(&MF_MT_FRAME_SIZE, (width as u64) << 32 | height as u64)
                    .map_err(|e| e.to_string())?;
                reader.SetCurrentMediaType(STREAM, None, &mt).map_err(|e| {
                    format!("{}: can't deliver RGB32 {width}x{height}: {e}", dev.name)
                })?;
                let cur = reader
                    .GetCurrentMediaType(STREAM)
                    .map_err(|e| e.to_string())?;
                let size = cur
                    .GetUINT64(&MF_MT_FRAME_SIZE)
                    .map_err(|e| e.to_string())?;
                let (width, height) = ((size >> 32) as u32, size as u32);
                let stride = match cur.GetUINT32(&MF_MT_DEFAULT_STRIDE) {
                    Ok(s) => s as i32 as isize,
                    Err(_) => -(width as isize * 4),
                };
                Ok(Camera {
                    reader,
                    width,
                    height,
                    stride,
                    _mf: mf,
                })
            }
        }

        /// The next frame. The source reader blocks until one arrives;
        /// `_timeout` is honoured by V4L2 only.
        pub fn read(&mut self, _timeout: Duration) -> Result<Raster, String> {
            // SAFETY: synchronous ReadSample; the buffer is unlocked below.
            unsafe {
                loop {
                    let mut flags = 0u32;
                    let mut sample = None;
                    self.reader
                        .ReadSample(STREAM, 0, None, Some(&mut flags), None, Some(&mut sample))
                        .map_err(|e| format!("camera read: {e}"))?;
                    if flags & (MF_SOURCE_READERF_ERROR.0 | MF_SOURCE_READERF_ENDOFSTREAM.0) as u32
                        != 0
                    {
                        return Err("the camera stopped sending frames".into());
                    }
                    let Some(sample) = sample else { continue };
                    let buf = sample
                        .ConvertToContiguousBuffer()
                        .map_err(|e| e.to_string())?;
                    if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
                        let mut p = std::ptr::null_mut();
                        let mut pitch = 0i32;
                        b2.Lock2D(&mut p, &mut pitch).map_err(|e| e.to_string())?;
                        let step = pitch.unsigned_abs() as usize;
                        let h = self.height as usize;
                        let (start, len) = if pitch < 0 {
                            (p.offset(pitch as isize * (h as isize - 1)), step * h)
                        } else {
                            (p, step * h)
                        };
                        let bytes = std::slice::from_raw_parts(start as *const u8, len);
                        let r = super::bgrx_to_rgba(bytes, self.width, self.height, pitch as isize);
                        let _ = b2.Unlock2D();
                        return r;
                    }
                    let mut p = std::ptr::null_mut();
                    let mut len = 0u32;
                    buf.Lock(&mut p, None, Some(&mut len))
                        .map_err(|e| e.to_string())?;
                    let bytes = std::slice::from_raw_parts(p as *const u8, len as usize);
                    let r = super::bgrx_to_rgba(bytes, self.width, self.height, self.stride);
                    let _ = buf.Unlock();
                    return r;
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn dev(name: &str, path: &str) -> Device {
        Device {
            name: name.into(),
            path: path.into(),
        }
    }

    #[test]
    fn choose_picks_first_or_by_path_or_name() {
        let d = [dev("Front Cam", "/dev/video0"), dev("C920", "/dev/video2")];
        assert_eq!(choose(&d, "").unwrap().path, "/dev/video0");
        assert_eq!(choose(&d, "  ").unwrap().path, "/dev/video0");
        assert_eq!(choose(&d, "/dev/video2").unwrap().name, "C920");
        assert_eq!(choose(&d, "c920").unwrap().path, "/dev/video2");
        let e = choose(&d, "Logi").unwrap_err();
        assert!(
            e.contains("'Logi' not found") && e.contains("Front Cam, C920"),
            "{e}"
        );
        assert!(choose(&[], "").unwrap_err().contains("no camera found"));
        assert!(choose(&[], "x").unwrap_err().contains("available: none"));
    }

    #[test]
    fn yuyv_converts_bt601_limited_range_and_honours_stride() {
        // 2x2: row 0 is white (Y=235) then black (Y=16), grey chroma;
        // row 1 is pure red-ish (Y=81 U=90 V=240). Rows padded to 6 bytes.
        let src = [
            235, 128, 16, 128, 0xAA, 0xAA, //
            81, 90, 81, 240, 0xAA, 0xAA,
        ];
        let r = yuyv_to_rgba(&src, 2, 2, 6).unwrap();
        assert_eq!((r.width, r.height), (2, 2));
        assert_eq!(&r.data[0..4], &[255, 255, 255, 255]);
        assert_eq!(&r.data[4..8], &[0, 0, 0, 255]);
        let red = &r.data[8..12];
        assert!(red[0] >= 250 && red[1] <= 5 && red[2] <= 5, "{red:?}");
        assert_eq!(&r.data[8..12], &r.data[12..16]);
        assert!(yuyv_to_rgba(&src, 3, 2, 6).is_err(), "odd width");
        assert!(yuyv_to_rgba(&src, 2, 2, 3).is_err(), "stride < width*2");
        assert!(yuyv_to_rgba(&src[..9], 2, 2, 6).is_err(), "short frame");
    }

    #[test]
    fn bgrx_swaps_channels_and_flips_bottom_up_rows() {
        // two rows, one pixel each, 8-byte pitch: blue then green
        let src = [255, 0, 0, 7, 0xEE, 0xEE, 0xEE, 0xEE, 0, 255, 0, 7];
        let top = bgrx_to_rgba(&src, 1, 2, 8).unwrap();
        assert_eq!(top.data, [0, 0, 255, 255, 0, 255, 0, 255]);
        // negative pitch: the bottom row comes first, so the image flips
        let up = bgrx_to_rgba(&src, 1, 2, -8).unwrap();
        assert_eq!(up.data, [0, 255, 0, 255, 0, 0, 255, 255]);
        assert!(bgrx_to_rgba(&src, 3, 2, 8).is_err(), "pitch < width*4");
        assert!(bgrx_to_rgba(&src[..10], 1, 2, 8).is_err(), "short frame");
    }

    /// Frames recorded from a camera (tests/fixtures/camera), decoded by
    /// [`decode`] like a V4L2 capture and played in a loop. After `ok`
    /// frames (never, if `None`) it fails with "unplugged".
    pub(crate) struct Recorded {
        packing: Packing,
        frames: Vec<&'static [u8]>,
        next: usize,
        pub ok: Option<u32>,
    }

    const RED_MJPEG: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/camera/red_320x180.mjpeg"
    ));
    const TESTSRC_YUYV: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/camera/testsrc_64x48.yuyv"
    ));

    impl Recorded {
        /// Solid red (200, 40, 40) 320x180 MJPEG, as a webcam sends it.
        pub(crate) fn red_mjpeg(ok: Option<u32>) -> Recorded {
            Recorded {
                packing: Packing::Mjpeg,
                frames: jpeg_frames(RED_MJPEG),
                next: 0,
                ok,
            }
        }

        /// ffmpeg's test pattern, 64x48 packed YUYV.
        pub(crate) fn testsrc_yuyv(ok: Option<u32>) -> Recorded {
            let packing = Packing::Yuyv {
                width: 64,
                height: 48,
                stride: 128,
            };
            let (frames, rest) = TESTSRC_YUYV.as_chunks::<{ 128 * 48 }>();
            assert!(rest.is_empty(), "truncated YUYV fixture");
            Recorded {
                packing,
                frames: frames.iter().map(|f| f.as_slice()).collect(),
                next: 0,
                ok,
            }
        }

        pub(crate) fn decoded(&self) -> Vec<Raster> {
            self.frames
                .iter()
                .map(|f| decode(self.packing, f).unwrap())
                .collect()
        }
    }

    /// Splits a `-f mjpeg` stream into its JPEGs: each ends at the EOI
    /// marker (FF D9), which byte stuffing keeps out of the image data.
    fn jpeg_frames(mut s: &'static [u8]) -> Vec<&'static [u8]> {
        let mut out = Vec::new();
        while let Some(end) = s.windows(2).position(|w| w == [0xFF, 0xD9]) {
            out.push(&s[..end + 2]);
            s = &s[end + 2..];
        }
        assert!(s.is_empty() && !out.is_empty(), "truncated MJPEG fixture");
        out
    }

    impl Read for Recorded {
        fn next_frame(&mut self, _: Duration) -> Result<Raster, String> {
            std::thread::sleep(Duration::from_millis(10));
            match &mut self.ok {
                Some(0) => return Err("unplugged".into()),
                Some(n) => *n -= 1,
                None => {}
            }
            let f = self.frames[self.next % self.frames.len()];
            self.next += 1;
            decode(self.packing, f)
        }
    }

    #[test]
    fn recorded_frames_decode_to_the_recorded_picture() {
        let red = Recorded::red_mjpeg(None).decoded();
        assert_eq!(red.len(), 5);
        for r in &red {
            assert_eq!((r.width, r.height), (320, 180));
            for px in r.data.as_chunks::<4>().0 {
                let d = px
                    .iter()
                    .zip([200u8, 40, 40, 255])
                    .map(|(a, b)| a.abs_diff(b));
                assert!(d.max().unwrap() <= 3, "{px:?}");
            }
        }
        // testsrc's top-left corner is in a solid colour block; YUYV
        // leaves luma per pixel, so its first two pixels match.
        let yuyv = Recorded::testsrc_yuyv(None).decoded();
        assert_eq!(yuyv.len(), 10);
        assert_eq!((yuyv[0].width, yuyv[0].height), (64, 48));
        assert_eq!(yuyv[0].data[..4], yuyv[0].data[4..8]);
        assert_ne!(yuyv[0].data, yuyv[9].data, "the pattern moves");
    }

    #[test]
    fn feed_reports_size_keeps_newest_frame_and_records_errors() {
        let cam = Recorded::testsrc_yuyv(Some(20));
        let frames = cam.decoded();
        let f = Feed::from_reader("recorded", cam).unwrap();
        assert_eq!(f.size, (64, 48));
        let latest = f.latest().unwrap();
        assert!(frames.iter().any(|r| r.data == latest.data));
        let t0 = std::time::Instant::now();
        while f.error().is_none() && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(f.error().as_deref(), Some("unplugged"));
        assert_eq!(f.frames_read(), 20);
    }

    #[test]
    fn feed_start_fails_loudly_when_the_camera_cannot_open_or_send() {
        let e = Feed::spawn::<Recorded>(dev("Gone", "x"), || Err("busy".into()))
            .err()
            .unwrap();
        assert_eq!(e, CameraError::Failed("camera 'Gone': busy".into()));
        let silent = Recorded::red_mjpeg(Some(0));
        let e = Feed::from_reader("Mute", silent).err().unwrap();
        assert_eq!(e, CameraError::Failed("camera 'Mute': unplugged".into()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn v4l2_ioctl_numbers_and_struct_sizes_match_videodev2_h() {
        use std::os::raw::c_ulong;
        use v4l2::*;
        assert_eq!(fourcc(b"YUYV"), 0x5659_5559);
        assert_eq!(PIX_MJPEG, 0x4750_4a4d);
        if cfg!(target_pointer_width = "64") {
            assert_eq!(size_of::<Capability>(), 104);
            assert_eq!(size_of::<Format>(), 208);
            assert_eq!(size_of::<RequestBuffers>(), 20);
            assert_eq!(size_of::<Buffer>(), 88);
            let want: [(c_ulong, c_ulong); 8] = [
                (QUERYCAP, 0x8068_5600),
                (S_FMT, 0xc0d0_5605),
                (REQBUFS, 0xc014_5608),
                (QUERYBUF, 0xc058_5609),
                (QBUF, 0xc058_560f),
                (DQBUF, 0xc058_5611),
                (STREAMON, 0x4004_5612),
                (STREAMOFF, 0x4004_5613),
            ];
            for (got, exp) in want {
                assert_eq!(got, exp, "{got:#x} != {exp:#x}");
            }
        }
        assert_eq!(
            CAP_VIDEO_CAPTURE | CAP_STREAMING | CAP_DEVICE_CAPS,
            0x8400_0001
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn v4l2_refuses_a_node_that_is_not_a_camera() {
        let e = v4l2::Camera::open_path("/dev/null", 640, 480)
            .err()
            .unwrap();
        assert!(
            e.to_string()
                .starts_with("/dev/null is not a V4L2 device ("),
            "{e}"
        );
        let e = v4l2::Camera::open_path("/nonexistent/video9", 640, 480)
            .err()
            .unwrap();
        assert!(
            matches!(&e, CameraError::Failed(m) if m.starts_with("can't open /nonexistent/video9")),
            "{e:?}"
        );
        // enumeration works (and is empty without a camera)
        assert!(list().is_ok());
    }

    /// A videoN node the user may not open is reported, not skipped: the
    /// listing names the node and the fix. A readable node that is not a
    /// camera is still skipped quietly.
    #[cfg(target_os = "linux")]
    #[test]
    fn v4l2_list_reports_a_node_it_is_not_allowed_to_open() {
        if rerun_as_nobody("camera::tests::v4l2_list_reports_a_node_it_is_not_allowed_to_open") {
            return;
        }
        let dir = crate::test_dir::TestDir::new("v4l2_nodes");
        let locked = locked_node(&dir, "video0");
        std::fs::write(dir.path().join("video1"), b"").unwrap();
        std::fs::write(dir.path().join("videoX"), b"").unwrap();
        let why = blocked_why(&locked);
        // blocked, not absent: listing and opening both say why
        let l = v4l2::list_in(dir.path()).unwrap();
        assert_eq!(
            l,
            Listing {
                blocked: vec![unusable(&locked, &why)],
                ..Listing::default()
            }
        );
        assert_eq!(l.pick(""), Err(CameraError::Blocked(why.clone())));
        let opened = Camera::open_path(&locked.to_string_lossy(), 64, 48).err();
        assert_eq!(opened, Some(CameraError::Blocked(why)));
        // absent: nothing to approve, just "No camera found"
        std::fs::remove_file(&locked).unwrap();
        assert_eq!(v4l2::list_in(dir.path()), Ok(Listing::default()));
    }

    /// A usable camera followed by real probed nodes.
    #[cfg(target_os = "linux")]
    pub(crate) fn mixed_listing(
        usable: Device,
        probes: Vec<(String, Result<Option<Device>, CameraError>)>,
    ) -> Listing {
        Listing::from_probes(std::iter::once((usable.path.clone(), Ok(Some(usable)))).chain(probes))
    }

    /// File modes don't bind root, so as root a permission test runs again
    /// in a child process as `nobody` (setgid + setuid; std drops the
    /// supplementary groups). True when that child ran `test` and it
    /// passed; the caller then returns. Panics if privileges can't be
    /// dropped or the child fails: never a quiet skip.
    #[cfg(target_os = "linux")]
    pub(crate) fn rerun_as_nobody(test: &str) -> bool {
        use std::os::unix::process::CommandExt;
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            return false;
        }
        // SAFETY: a NUL-terminated name; the record is read at once.
        let (uid, gid) = unsafe {
            let pw = libc::getpwnam(c"nobody".as_ptr());
            assert!(
                !pw.is_null(),
                "running as root, and there is no `nobody` user to drop to: {test} needs a user that file modes apply to"
            );
            ((*pw).pw_uid, (*pw).pw_gid)
        };
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new(&exe)
            .args([test, "--exact", "--nocapture", "--test-threads=1"])
            .uid(uid)
            .gid(gid)
            .current_dir("/")
            .output()
            .unwrap_or_else(|e| {
                panic!(
                    "running as root: can't drop to nobody ({uid}:{gid}) to run {test} from {}: {e}",
                    exe.display()
                )
            });
        let text = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success() && text.contains("test result: ok. 1 passed"),
            "{test} as nobody ({uid}:{gid}) did not pass:\n{text}"
        );
        eprintln!("{test}: this process is root, so it ran as nobody ({uid}:{gid}) and passed");
        true
    }

    /// A mode-000 `name` in `dir`, which this user can't open.
    #[cfg(target_os = "linux")]
    pub(crate) fn locked_node(dir: &crate::test_dir::TestDir, name: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let locked = dir.path().join(name);
        std::fs::write(&locked, b"").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(
            std::fs::File::open(&locked).is_err(),
            "{} opens anyway: this test needs a user that file modes apply to (not root)",
            locked.display()
        );
        locked
    }

    #[cfg(target_os = "linux")]
    fn unusable(node: &std::path::Path, why: &str) -> Unusable {
        Unusable {
            path: node.to_string_lossy().into_owned(),
            why: why.into(),
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn blocked_why(node: &std::path::Path) -> String {
        format!(
            "Permission denied on {}: your user isn't allowed to use cameras (not in the video group)",
            node.display()
        )
    }

    /// Usable and blocked cameras together: the usable ones are listed and
    /// each blocked one is named; nothing is hidden. The nodes are real: a
    /// readable one (opened, then skipped: a plain file answers no
    /// VIDIOC_QUERYCAP) and an unreadable one. No V4L2 camera exists on
    /// the dev box, so the usable camera is a Device as `query` returns it.
    #[cfg(target_os = "linux")]
    #[test]
    fn v4l2_list_shows_usable_and_blocked_cameras_together() {
        if rerun_as_nobody("camera::tests::v4l2_list_shows_usable_and_blocked_cameras_together") {
            return;
        }
        let dir = crate::test_dir::TestDir::new("v4l2_mixed");
        let readable = dir.path().join("video1");
        std::fs::write(&readable, b"").unwrap();
        let locked = locked_node(&dir, "video2");
        let locked2 = locked_node(&dir, "video3");
        let probes: Vec<_> = [&readable, &locked, &locked2]
            .iter()
            .map(|p| {
                let p = p.to_string_lossy().into_owned();
                let r = v4l2::probe(&p);
                (p, r)
            })
            .collect();
        assert_eq!(probes[0].1, Ok(None), "readable node opens");
        let usable = dev("USB Cam", "/dev/video0");
        let l = mixed_listing(usable.clone(), probes);
        assert_eq!(
            l,
            Listing {
                cameras: vec![usable.clone()],
                blocked: vec![
                    unusable(&locked, &blocked_why(&locked)),
                    unusable(&locked2, &blocked_why(&locked2)),
                ],
                failed: vec![],
            }
        );
        assert_eq!(
            l.blocked_why().unwrap(),
            format!("{}\n{}", blocked_why(&locked), blocked_why(&locked2))
        );
        assert_eq!(l.pick(""), Ok(&usable));
        assert_eq!(
            l.pick("Other"),
            Err(CameraError::Blocked(format!(
                "camera 'Other' not found (available: USB Cam); {}; {}",
                blocked_why(&locked),
                blocked_why(&locked2)
            )))
        );
        // and from the folder scan itself
        let scanned = v4l2::list_in(dir.path()).unwrap();
        assert_eq!(scanned.blocked, l.blocked);
    }

    #[test]
    fn windows_access_denied_at_activation_is_blocked_access() {
        let denied = 0x8007_0005_u32 as i32;
        assert_eq!(
            activate_error("USB Cam", denied, "Access is denied."),
            CameraError::Blocked(
                "Windows camera privacy settings block USB Cam (Let desktop apps access your camera is off)"
                    .into()
            )
        );
        let e_fail = 0x8000_4005_u32 as i32;
        assert_eq!(
            activate_error("USB Cam", e_fail, "busy"),
            CameraError::Failed("USB Cam: activate: busy".into())
        );
    }

    #[test]
    fn approve_on_windows_opens_the_camera_privacy_page() {
        let c = settings_command(WINDOWS_CAMERA_PRIVACY);
        assert_eq!(c.get_program(), "cmd");
        let args: Vec<_> = c.get_args().collect();
        assert_eq!(args, ["/C", "start", "", "ms-settings:privacy-webcam"]);
        let here = approve();
        if cfg!(windows) {
            assert_eq!(
                here,
                Some(Approve::OpenSettings("ms-settings:privacy-webcam"))
            );
        } else if cfg!(target_os = "linux") {
            assert!(
                matches!(here, Some(Approve::ShowFix { command, .. }) if command == "sudo usermod -aG video $USER"),
                "{here:?}"
            );
        } else {
            assert_eq!(here, None);
        }
    }

    /// Needs a real V4L2 node: `sudo modprobe v4l2loopback devices=1` and a
    /// producer such as `ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30
    /// -pix_fmt yuyv422 -f v4l2 /dev/video0`, then
    /// `SERAPHINA_TEST_CAMERA=/dev/video0 cargo test --bin ui -- --ignored v4l2_reads`.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "needs a V4L2 device (v4l2loopback); the dev box's Firecracker kernel has no media support"]
    fn v4l2_reads_frames_from_a_real_device() {
        let path = std::env::var("SERAPHINA_TEST_CAMERA").unwrap_or("/dev/video0".into());
        let d = list().unwrap().pick(&path).unwrap().clone();
        let f = Feed::start(&d.path, 640, 480).unwrap();
        assert!(f.size.0 > 0 && f.size.1 > 0);
        std::thread::sleep(Duration::from_secs(1));
        assert!(f.frames_read() >= 10, "{} frames in 1 s", f.frames_read());
        assert!(f.error().is_none(), "{:?}", f.error());
        let r = f.latest().unwrap();
        assert_eq!(r.data.len(), (r.width * r.height * 4) as usize);
        assert!(r
            .data
            .chunks(4)
            .any(|p| p[0] > 32 || p[1] > 32 || p[2] > 32));
    }

    /// Media Foundation enumeration runs on Windows (and under Wine, which
    /// has no capture devices, so the list is empty but must not fail).
    #[cfg(windows)]
    #[test]
    fn media_foundation_lists_cameras() {
        let l = list().expect("MFEnumDeviceSources");
        assert!(l.blocked.is_empty() && l.failed.is_empty(), "{l:?}");
        for d in &l.cameras {
            assert!(!d.name.is_empty() && !d.path.is_empty(), "{d:?}");
        }
    }

    /// Opens the first real camera through Media Foundation.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs a real webcam; Wine has none. Run on the Windows PC with --ignored"]
    fn media_foundation_reads_frames_from_a_real_camera() {
        let f = Feed::start("", 640, 480).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        assert!(f.frames_read() >= 5, "{} frames in 1 s", f.frames_read());
        let r = f.latest().unwrap();
        assert_eq!(r.data.len(), (r.width * r.height * 4) as usize);
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    #[test]
    fn other_systems_refuse_cameras() {
        assert!(list().unwrap_err().contains("isn't supported"));
    }
}
