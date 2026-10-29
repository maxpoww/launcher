//! A GPU buffer the CPU can read, for the screen sampler (`screencopy.rs`).
//!
//! The sampler used to capture into shared memory. For shm Hyprland renders
//! the screen into a scratch framebuffer and then READS IT BACK with
//! `glReadPixels` — a synchronous wait for the whole GPU queue — before it
//! can draw its next frame. Measured on the dev box (3200x2000, 2026-10-28,
//! golem-stress): 30–70 ms of frozen compositor per capture, twice per
//! sample interval whenever the screen is changing — ~20 dropped frames a
//! second during any animation, video or recording. A dma-buf capture is
//! rendered by the GPU straight into our buffer and `ready` comes when the
//! GPU is done: the compositor never waits.
//!
//! The buffer is a DRM "dumb" buffer: linear, 32 bpp, allocated on the
//! compositor's own card through two ioctls, exported as a dma-buf for the
//! compositor (`zwp_linux_dmabuf_v1`) and mapped here for reading. No GBM, no
//! new crates: `libc` ioctls on `/dev/dri/cardN` (the card node — render nodes
//! do not do dumb buffers; the logged-in user holds it through logind's ACL).
//! Reads are bracketed with `DMA_BUF_IOCTL_SYNC` so the GPU's writes are
//! visible. Anything that fails here makes the sampler fall back to shm.
use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;

/// `DRM_FORMAT_XRGB8888` / `DRM_FORMAT_ARGB8888` (fourcc, little endian).
pub(crate) const FORMAT_XRGB8888: u32 = 0x3432_5258;
pub(crate) const FORMAT_ARGB8888: u32 = 0x3432_5241;
/// `DRM_FORMAT_MOD_LINEAR`.
pub(crate) const MODIFIER_LINEAR: u64 = 0;

// drm.h / drm_mode.h, x86_64 ioctl numbers.
const DRM_IOCTL_MODE_CREATE_DUMB: libc::c_ulong = 0xC020_64B2;
const DRM_IOCTL_MODE_MAP_DUMB: libc::c_ulong = 0xC010_64B3;
const DRM_IOCTL_MODE_DESTROY_DUMB: libc::c_ulong = 0xC004_64B4;
const DRM_IOCTL_PRIME_HANDLE_TO_FD: libc::c_ulong = 0xC00C_642D;
const DRM_CLOEXEC: u32 = libc::O_CLOEXEC as u32;
// linux/dma-buf.h
const DMA_BUF_IOCTL_SYNC: libc::c_ulong = 0x4008_6200;
const DMA_BUF_SYNC_READ: u64 = 1;
const DMA_BUF_SYNC_END: u64 = 4;

#[repr(C)]
struct CreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}
#[repr(C)]
struct MapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}
#[repr(C)]
struct DestroyDumb {
    handle: u32,
}
#[repr(C)]
struct PrimeHandle {
    handle: u32,
    flags: u32,
    fd: i32,
}
#[repr(C)]
struct DmaBufSync {
    flags: u64,
}

/// The card the compositor draws with: the boot GPU (`boot_vga`), else the
/// first card. (Hyprland without `AQ_DRM_DEVICES` picks the same.)
pub(crate) fn compositor_card() -> Option<std::path::PathBuf> {
    let mut cards: Vec<_> = std::fs::read_dir("/dev/dri")
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("card") && n[4..].chars().all(|c| c.is_ascii_digit()))
        .collect();
    cards.sort();
    let boot = cards.iter().find(|n| {
        std::fs::read_to_string(format!("/sys/class/drm/{n}/device/boot_vga"))
            .is_ok_and(|v| v.trim() == "1")
    });
    boot.or(cards.first()).map(|n| std::path::PathBuf::from("/dev/dri").join(n))
}

/// One linear 32-bpp buffer on a card, exported as a dma-buf and mapped.
pub(crate) struct DumbBuffer {
    card: OwnedFd,
    handle: u32,
    /// The dma-buf: what the compositor imports.
    pub(crate) fd: OwnedFd,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) stride: u32,
    size: usize,
    map: *mut u8,
}

// SAFETY: the mapping is private to this struct and only read through
// `read`, which takes `&self`; fds are owned.
unsafe impl Send for DumbBuffer {}

impl DumbBuffer {
    pub(crate) fn create(card_path: &std::path::Path, width: u32, height: u32) -> io::Result<Self> {
        let card = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(card_path)?;
        let card: OwnedFd = card.into();
        let mut req = CreateDumb { height, width, bpp: 32, flags: 0, handle: 0, pitch: 0, size: 0 };
        // SAFETY: a correctly sized, repr(C) request on an open DRM fd.
        if unsafe { libc::ioctl(card.as_raw_fd(), DRM_IOCTL_MODE_CREATE_DUMB, &mut req) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let handle = req.handle;
        let destroy = |card: RawFd| {
            let mut d = DestroyDumb { handle };
            // SAFETY: as above.
            unsafe { libc::ioctl(card, DRM_IOCTL_MODE_DESTROY_DUMB, &mut d) };
        };
        let mut prime = PrimeHandle { handle, flags: DRM_CLOEXEC | libc::O_RDWR as u32, fd: -1 };
        // SAFETY: as above.
        if unsafe { libc::ioctl(card.as_raw_fd(), DRM_IOCTL_PRIME_HANDLE_TO_FD, &mut prime) } != 0 {
            let e = io::Error::last_os_error();
            destroy(card.as_raw_fd());
            return Err(e);
        }
        // SAFETY: the kernel just handed us this fd; we own it from here.
        let fd = unsafe { OwnedFd::from_raw_fd(prime.fd) };
        let mut mapreq = MapDumb { handle, pad: 0, offset: 0 };
        // SAFETY: as above.
        if unsafe { libc::ioctl(card.as_raw_fd(), DRM_IOCTL_MODE_MAP_DUMB, &mut mapreq) } != 0 {
            let e = io::Error::last_os_error();
            destroy(card.as_raw_fd());
            return Err(e);
        }
        let size = req.size as usize;
        // SAFETY: a read-only shared mapping of the dumb buffer at the offset
        // the driver gave for it; unmapped in Drop.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ,
                libc::MAP_SHARED,
                card.as_raw_fd(),
                mapreq.offset as libc::off_t,
            )
        };
        if map == libc::MAP_FAILED {
            let e = io::Error::last_os_error();
            destroy(card.as_raw_fd());
            return Err(e);
        }
        Ok(Self { card, handle, fd, width, height, stride: req.pitch, size, map: map.cast() })
    }

    /// Before reading: the GPU's writes made visible to the CPU.
    pub(crate) fn begin_read(&self) {
        let mut sync = DmaBufSync { flags: DMA_BUF_SYNC_READ };
        // SAFETY: the sync ioctl on our own dma-buf; a failure only costs
        // the coherence of one sample.
        unsafe { libc::ioctl(self.fd.as_raw_fd(), DMA_BUF_IOCTL_SYNC, &mut sync) };
    }

    /// After reading.
    pub(crate) fn end_read(&self) {
        let mut sync = DmaBufSync { flags: DMA_BUF_SYNC_READ | DMA_BUF_SYNC_END };
        // SAFETY: as above.
        unsafe { libc::ioctl(self.fd.as_raw_fd(), DMA_BUF_IOCTL_SYNC, &mut sync) };
    }

    /// The buffer's bytes: rows of `stride`, between `begin_read` and `end_read`.
    pub(crate) fn bytes(&self) -> &[u8] {
        // SAFETY: `map` is a live read-only mapping of `size` bytes for the
        // life of self.
        unsafe { std::slice::from_raw_parts(self.map, self.size) }
    }
}

impl Drop for DumbBuffer {
    fn drop(&mut self) {
        // SAFETY: undoing exactly what `create` did.
        unsafe {
            libc::munmap(self.map.cast(), self.size);
            let mut d = DestroyDumb { handle: self.handle };
            libc::ioctl(self.card.as_raw_fd(), DRM_IOCTL_MODE_DESTROY_DUMB, &mut d);
        }
    }
}
