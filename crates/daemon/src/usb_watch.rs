//! USB devices coming and going, as THE KERNEL tells it (its uevent socket).
//!
//! The mounts' and the scanners' workers read `/sys/bus/usb/devices` every 3 s
//! to notice a device: 20 file reads a second and a wake-up every 3 s of a dock
//! at rest, on every machine, for an event of a few times a week (2026-10-30).
//! The kernel says it the moment it happens, to anyone listening, with no
//! privileges and no daemon. Each worker keeps a slow look of its own behind
//! this, for the one thing a socket cannot promise (a burst lost when its queue
//! was full, a socket refused).

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Mutex;

use tracing::{debug, warn};

type Listener = Box<dyn FnMut() -> bool + Send>;

/// Who wants to know. A listener answers whether it still does.
static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());
/// The socket's thread started, and whether it stands.
static STARTED: Mutex<Option<bool>> = Mutex::new(None);

/// `told` runs (on the socket's thread: it should only pass the word on) each
/// time a USB device or interface comes or goes. False: nobody will be told
/// here; the caller keeps looking by itself.
pub(crate) fn listen(told: impl FnMut() -> bool + Send + 'static) -> bool {
    let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
    let stands = *started.get_or_insert_with(start);
    if stands {
        LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).push(Box::new(told));
    }
    stands
}

/// Whether a kernel event is a USB device or interface coming or going
/// (`add@/devices/…\0ACTION=add\0…SUBSYSTEM=usb\0…`).
fn is_usb_change(event: &[u8]) -> bool {
    let coming_or_going = [&b"add@"[..], b"remove@", b"bind@", b"unbind@"].iter().any(|a| event.starts_with(a));
    coming_or_going && event.split(|b| *b == 0).any(|field| field == b"SUBSYSTEM=usb")
}

fn start() -> bool {
    let Some(socket) = open() else {
        debug!("usb: no uevent socket; the workers look for themselves");
        return false;
    };
    let spawned = std::thread::Builder::new().name("usb-watch".into()).spawn(move || {
        // The kernel's events are small (under 8 KiB by its own limit).
        let mut event = [0u8; 8192];
        loop {
            // SAFETY: the buffer is ours and as long as we say.
            let n = unsafe { libc::recv(socket.as_raw_fd(), event.as_mut_ptr().cast(), event.len(), 0) };
            let usb = if n < 0 {
                match std::io::Error::last_os_error().raw_os_error() {
                    Some(libc::EINTR) => continue,
                    // Its queue overflowed: something happened, and it may be this.
                    Some(libc::ENOBUFS) => true,
                    _ => {
                        warn!("usb: the uevent socket ended; the workers' slow looks remain");
                        return;
                    }
                }
            } else {
                is_usb_change(&event[..n as usize])
            };
            if usb {
                LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).retain_mut(|told| told());
            }
        }
    });
    spawned.is_ok()
}

/// The kernel's uevent socket, group 1 (its own events, not udev's retelling).
fn open() -> Option<OwnedFd> {
    // SAFETY: plain system calls; the descriptor is owned from the moment it
    // exists and the address is a zeroed `sockaddr_nl` with its fields set.
    unsafe {
        let fd = libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_KOBJECT_UEVENT,
        );
        if fd < 0 {
            return None;
        }
        let socket = OwnedFd::from_raw_fd(fd);
        let mut addr: libc::sockaddr_nl = std::mem::zeroed();
        addr.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        addr.nl_groups = 1;
        let bound = libc::bind(
            socket.as_raw_fd(),
            std::ptr::from_ref(&addr).cast(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        );
        (bound == 0).then_some(socket)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_usb_coming_or_going_counts() {
        let usb = b"add@/devices/pci0000:00/usb3/3-9\0ACTION=add\0DEVPATH=/x\0SUBSYSTEM=usb\0DEVTYPE=usb_device\0";
        assert!(is_usb_change(usb));
        assert!(is_usb_change(b"unbind@/devices/x/3-9:1.0\0ACTION=unbind\0SUBSYSTEM=usb\0"));
        assert!(!is_usb_change(b"change@/devices/x/3-9\0ACTION=change\0SUBSYSTEM=usb\0"));
        assert!(!is_usb_change(b"add@/devices/virtual/net/x\0ACTION=add\0SUBSYSTEM=net\0"));
        // Not fooled by a field that only begins or holds the same words.
        assert!(!is_usb_change(b"add@/devices/x\0ACTION=add\0SUBSYSTEM=usbmisc\0"));
        assert!(!is_usb_change(b"add@/devices/SUBSYSTEM=usb\0ACTION=add\0SUBSYSTEM=block\0"));
    }
}
