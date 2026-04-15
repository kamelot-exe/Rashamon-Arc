//! DRM/KMS direct display — bypasses X11/Wayland completely.
//!
//! Uses DRM ioctls to:
//! 1. Open /dev/dri/card0
//! 2. Get connectors, CRTCs, encoders
//! 3. Find the best mode
//! 4. Create a dumb buffer + mmap it
//! 5. Set the mode (modeset)
//! 6. Page-flip on vsync

use std::fs::{File, OpenOptions};
use std::io;
use std::mem;
use std::os::unix::io::{AsRawFd, RawFd};

use rashamon_renderer::Framebuffer;

use libc::{
    c_int, c_uint, c_void, off_t,
    ioctl, mmap, munmap, MAP_SHARED,
};

const PROT_RW: i32 = libc::PROT_READ | libc::PROT_WRITE;

// ── DRM ioctl numbers ─────────────────────────────────────────────────────

const DRM_IOCTL_BASE: u32 = 0x64;
fn drm_ioc(nr: u32, sz: u32) -> u32 {
    0xC000_0000 | (sz << 16) | (DRM_IOCTL_BASE << 8) | nr
}

const DRM_IOCTL_MODE_GETRESOURCES: u32 = drm_ioc(0xA0, mem::size_of::<drm_mode_card_res>() as u32);
const DRM_IOCTL_MODE_GETCONNECTOR: u32 = drm_ioc(0xA7, mem::size_of::<drm_mode_get_connector>() as u32);
const DRM_IOCTL_MODE_GETCRTC: u32 = drm_ioc(0xA1, mem::size_of::<drm_mode_crtc>() as u32);
const DRM_IOCTL_MODE_SETMODESET: u32 = drm_ioc(0xA2, mem::size_of::<drm_mode_mode_set>() as u32);
const DRM_IOCTL_MODE_PAGE_FLIP: u32 = drm_ioc(0xAE, mem::size_of::<drm_mode_crtc_page_flip>() as u32);
const DRM_IOCTL_MODE_CREATE_DUMB: u32 = drm_ioc(0xB0, mem::size_of::<drm_mode_create_dumb>() as u32);
const DRM_IOCTL_MODE_MAP_DUMB: u32 = drm_ioc(0xB1, mem::size_of::<drm_mode_map_dumb>() as u32);
const DRM_IOCTL_MODE_DESTROY_DUMB: u32 = drm_ioc(0xB2, mem::size_of::<drm_mode_destroy_dumb>() as u32);
const DRM_IOCTL_MODE_GETENCODER: u32 = drm_ioc(0xA6, mem::size_of::<drm_mode_get_encoder>() as u32);

// ── Pixel format ───────────────────────────────────────────────────────────

const DRM_FORMAT_XRGB8888: u32 = 0x34325258; // "XR24" little-endian

// ── DRM structures (matching kernel ABI) ──────────────────────────────────

#[repr(C)]
struct drm_mode_card_res {
    fb_id_ptr: u64,
    crtcs_ptr: u64,
    connectors_ptr: u64,
    encoders_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
struct drm_mode_get_connector {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

#[repr(C)]
struct drm_mode_get_encoder {
    encoder_id: u32,
    encoder_type: u32,
    crtc_id: u32,
    possible_crtcs: u32,
    possible_clones: u32,
}

#[repr(C)]
struct drm_mode_modeinfo {
    clock: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    vrefresh: u32,
    flags: u32,
    type_: u32,
    name: [i8; 32],
}

#[repr(C)]
struct drm_mode_crtc {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    mode_ptr: u64,
    mode_valid: u32,
    gamma_size: u32,
}

#[repr(C)]
struct drm_mode_mode_set {
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    connectors_ptr: u64,
    count_connectors: u32,
    mode_ptr: u64,
}

#[repr(C)]
struct drm_mode_crtc_page_flip {
    fb_id: u32,
    flags: u32,
    crtc_id: u32,
    reserved: u32,
    user_data: u64,
}

const DRM_MODE_PAGE_FLIP_EVENT: u32 = 0x01;

#[repr(C)]
struct drm_mode_create_dumb {
    width: u32,
    height: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

#[repr(C)]
struct drm_mode_map_dumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

#[repr(C)]
struct drm_mode_destroy_dumb {
    handle: u32,
}

// ── IOCTL helper ───────────────────────────────────────────────────────────

unsafe fn drm_ioctl(fd: RawFd, request: u32, arg: *mut c_void) -> io::Result<()> {
    let ret = ioctl(fd, request as c_uint, arg);
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

// ── Display ────────────────────────────────────────────────────────────────

/// DRM/KMS display output.
pub struct Display {
    fd: File,
    width: u32,
    height: u32,
    crtc_id: u32,
    connector_id: u32,
    fb_id: u32,
    mode: drm_mode_modeinfo,
    /// Mapped dumb buffer (raw pointer).
    fb_ptr: *mut u8,
    fb_size: u64,
    fb_pitch: u32,
    gem_handle: u32,
    frame_count: u64,
}

impl Display {
    pub fn new(width: u32, height: u32) -> io::Result<Self> {
        let fd = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/dri/card0")?;

        let display = unsafe { Self::init(fd, width, height)? };
        eprintln!(
            "[display] DRM/KMS: {}x{} @ {}Hz (card0, fb pitch={})",
            display.width, display.height, display.mode.vrefresh, display.fb_pitch
        );
        Ok(display)
    }

    unsafe fn init(fd: File, req_width: u32, req_height: u32) -> io::Result<Self> {
        let raw_fd = fd.as_raw_fd();

        // 1. Get card resources.
        let mut res = drm_mode_card_res {
            fb_id_ptr: 0, crtcs_ptr: 0, connectors_ptr: 0, encoders_ptr: 0,
            count_fbs: 0, count_crtcs: 0, count_connectors: 0, count_encoders: 0,
            min_width: 0, max_width: 0, min_height: 0, max_height: 0,
        };
        drm_ioctl(raw_fd, DRM_IOCTL_MODE_GETRESOURCES, &mut res as *mut _ as _)?;

        if res.count_connectors == 0 {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no connectors found"));
        }

        // Fetch connector IDs.
        let mut conn_ids = vec![0u32; res.count_connectors as usize];
        res.connectors_ptr = conn_ids.as_mut_ptr() as u64;
        drm_ioctl(raw_fd, DRM_IOCTL_MODE_GETRESOURCES, &mut res as *mut _ as _)?;

        // 2. Find a connected connector with a matching mode.
        let mut connector_id = 0u32;
        let mut best_mode: Option<drm_mode_modeinfo> = None;
        let mut crtc_id = 0u32;

        for &cid in &conn_ids {
            // Probe connector.
            let mut conn = drm_mode_get_connector {
                encoders_ptr: 0, modes_ptr: 0, props_ptr: 0, prop_values_ptr: 0,
                count_modes: 0, count_props: 0, count_encoders: 0,
                encoder_id: 0, connector_id: cid,
                connector_type: 0, connector_type_id: 0,
                connection: 0, mm_width: 0, mm_height: 0, subpixel: 0, pad: 0,
            };
            drm_ioctl(raw_fd, DRM_IOCTL_MODE_GETCONNECTOR, &mut conn as *mut _ as _)?;

            if conn.connection != 1 {
                // DRM_MODE_CONNECTED
                continue;
            }

            // Fetch modes.
            let mut modes = vec![drm_mode_modeinfo {
                clock: 0, hdisplay: 0, hsync_start: 0, hsync_end: 0, htotal: 0,
                hskew: 0, vdisplay: 0, vsync_start: 0, vsync_end: 0, vtotal: 0,
                vscan: 0, vrefresh: 0, flags: 0, type_: 0, name: [0; 32],
            }; conn.count_modes as usize];
            let mut enc_ids = vec![0u32; conn.count_encoders as usize];
            conn.modes_ptr = modes.as_mut_ptr() as u64;
            conn.encoders_ptr = enc_ids.as_mut_ptr() as u64;
            drm_ioctl(raw_fd, DRM_IOCTL_MODE_GETCONNECTOR, &mut conn as *mut _ as _)?;

            // Find best mode (prefer requested resolution).
            for mode in &modes {
                let mw = mode.hdisplay as u32;
                let mh = mode.vdisplay as u32;
                if mw == req_width && mh == req_height {
                    best_mode = Some(*mode);
                    break;
                }
            }
            if best_mode.is_none() && !modes.is_empty() {
                // Use the first mode (usually the preferred mode).
                best_mode = Some(modes[0]);
            }

            if let Some(mode) = best_mode {
                connector_id = cid;

                // Get encoder → CRTC.
                for &eid in &enc_ids {
                    let mut enc = drm_mode_get_encoder {
                        encoder_id: eid, encoder_type: 0, crtc_id: 0,
                        possible_crtcs: 0, possible_clones: 0,
                    };
                    drm_ioctl(raw_fd, DRM_IOCTL_MODE_GETENCODER, &mut enc as *mut _ as _)?;
                    if enc.crtc_id != 0 {
                        crtc_id = enc.crtc_id;
                        break;
                    }
                }

                // If no CRTC from encoder, try the first CRTC.
                if crtc_id == 0 && res.count_crtcs > 0 {
                    let mut crtc_ids = vec![0u32; res.count_crtcs as usize];
                    res.crtcs_ptr = crtc_ids.as_mut_ptr() as u64;
                    drm_ioctl(raw_fd, DRM_IOCTL_MODE_GETRESOURCES, &mut res as *mut _ as _)?;
                    crtc_id = crtc_ids[0];
                }

                break;
            }
        }

        if connector_id == 0 {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no connected connector found"));
        }

        let mode = best_mode.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no suitable mode found")
        })?;

        let width = mode.hdisplay as u32;
        let height = mode.vdisplay as u32;

        // 3. Create dumb buffer.
        let mut create = drm_mode_create_dumb {
            width,
            height,
            bpp: 32, // XRGB8888
            flags: 0,
            handle: 0,
            pitch: 0,
            size: 0,
        };
        drm_ioctl(raw_fd, DRM_IOCTL_MODE_CREATE_DUMB, &mut create as *mut _ as _)?;

        let gem_handle = create.handle;
        let fb_size = create.size;
        let fb_pitch = create.pitch;

        // 4. Map dumb buffer.
        let mut map = drm_mode_map_dumb {
            handle: gem_handle,
            pad: 0,
            offset: 0,
        };
        drm_ioctl(raw_fd, DRM_IOCTL_MODE_MAP_DUMB, &mut map as *mut _ as _)?;

        let fb_ptr = mmap(
            std::ptr::null_mut(),
            fb_size as usize,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            raw_fd,
            map.offset as off_t,
        );

        if fb_ptr == libc::MAP_FAILED {
            // Clean up dumb buffer on failure.
            let mut destroy = drm_mode_destroy_dumb { handle: gem_handle };
            let _ = drm_ioctl(raw_fd, DRM_IOCTL_MODE_DESTROY_DUMB, &mut destroy as *mut _ as _);
            return Err(io::Error::new(io::Error::last_os_error(), "mmap dumb buffer failed"));
        }

        // 5. Create DRM framebuffer.
        let mut fb_id = 0u32;
        let ret = drm_sys_drm_mode_addfb(
            raw_fd,
            width,
            height,
            fb_pitch,
            DRM_FORMAT_XRGB8888,
            gem_handle,
            &mut fb_id,
        );
        if ret.is_err() {
            let _ = munmap(fb_ptr, fb_size as usize);
            let mut destroy = drm_mode_destroy_dumb { handle: gem_handle };
            let _ = drm_ioctl(raw_fd, DRM_IOCTL_MODE_DESTROY_DUMB, &mut destroy as *mut _ as _);
            return Err(io::Error::new(io::ErrorKind::Other, "drmModeAddFB failed"));
        }

        // 6. Set the mode (modeset).
        let mut connectors = [connector_id];
        let mut mode_set = drm_mode_mode_set {
            crtc_id,
            fb_id,
            x: 0,
            y: 0,
            width,
            height,
            connectors_ptr: connectors.as_mut_ptr() as u64,
            count_connectors: 1,
            mode_ptr: &mode as *const _ as u64,
        };
        drm_ioctl(raw_fd, DRM_IOCTL_MODE_SETMODESET, &mut mode_set as *mut _ as _)?;

        eprintln!("[display] modeset: {}x{} @ {}Hz fb_id={}", width, height, mode.vrefresh, fb_id);

        Ok(Self {
            fd,
            width,
            height,
            crtc_id,
            connector_id,
            fb_id,
            mode,
            fb_ptr: fb_ptr as *mut u8,
            fb_size,
            fb_pitch,
            gem_handle,
            frame_count: 0,
        })
    }

    /// Present the framebuffer by copying from the software FB and page-flipping.
    pub fn present(&mut self, fb: &Framebuffer) -> io::Result<()> {
        self.frame_count += 1;

        // Copy the software framebuffer to the DRM dumb buffer.
        // Convert BGR → XRGB8888 and handle pitch differences.
        unsafe {
            let dst_pitch = self.fb_pitch as usize;
            let src_pitch = fb.stride as usize;
            let copy_width = (self.width as usize).min(fb.width as usize) * 3;
            let copy_height = (self.height as usize).min(fb.height as usize);

            for y in 0..copy_height {
                let src_row = fb.data.as_ptr().add(y * src_pitch);
                let dst_row = self.fb_ptr.add(y * dst_pitch);

                for x in 0..(copy_width / 3) {
                    let src_px = src_row.add(x * 3);
                    let dst_px = dst_row.add(x * 4);

                    // BGR → XRGB8888 (little-endian: B, G, R, X)
                    *dst_px = *src_px;         // B
                    *dst_px.offset(1) = *src_px.offset(1); // G
                    *dst_px.offset(2) = *src_px.offset(2); // R
                    *dst_px.offset(3) = 0;     // X (unused)
                }
            }
        }

        // Page flip on vsync.
        let flip = drm_mode_crtc_page_flip {
            fb_id: self.fb_id,
            flags: DRM_MODE_PAGE_FLIP_EVENT,
            crtc_id: self.crtc_id,
            reserved: 0,
            user_data: self.frame_count,
        };
        unsafe {
            drm_ioctl(self.fd.as_raw_fd(), DRM_IOCTL_MODE_PAGE_FLIP, &flip as *const _ as *mut _)?;
        }

        Ok(())
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        unsafe {
            // Unmap dumb buffer.
            let _ = munmap(self.fb_ptr as *mut c_void, self.fb_size as usize);

            // Destroy dumb buffer.
            let mut destroy = drm_mode_destroy_dumb { handle: self.gem_handle };
            let _ = drm_ioctl(self.fd.as_raw_fd(), DRM_IOCTL_MODE_DESTROY_DUMB, &mut destroy as *mut _ as _);
        }
    }
}

// ── drmModeAddFB wrapper (ioctl DRM_IOCTL_MODE_ADDFB) ─────────────────────

const DRM_IOCTL_MODE_ADDFB: u32 = drm_ioc(0xAD, mem::size_of::<drm_mode_fb_cmd>() as u32);

#[repr(C)]
struct drm_mode_fb_cmd {
    fb_id: u32,
    width: u32,
    height: u32,
    pitch: u32,
    bpp: u32,
    depth: u32,
    handle: u32,
}

unsafe fn drm_sys_drm_mode_addfb(
    fd: RawFd,
    width: u32,
    height: u32,
    pitch: u32,
    pixel_format: u32,
    gem_handle: u32,
    fb_id: &mut u32,
) -> io::Result<()> {
    // Map DRM_FORMAT_XRGB8888 to depth/bpp for the legacy ADDFB ioctl.
    let (depth, bpp) = match pixel_format {
        DRM_FORMAT_XRGB8888 => (24, 32),
        _ => (24, 32),
    };

    let mut cmd = drm_mode_fb_cmd {
        fb_id: 0,
        width,
        height,
        pitch,
        bpp,
        depth,
        handle: gem_handle,
    };
    drm_ioctl(fd, DRM_IOCTL_MODE_ADDFB, &mut cmd as *mut _ as _)?;
    *fb_id = cmd.fb_id;
    Ok(())
}
