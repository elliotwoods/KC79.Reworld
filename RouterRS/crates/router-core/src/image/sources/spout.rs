//! Spout receiver source (port of `Image/Sources/Spout.*`, Windows GPU
//! texture sharing).
//!
//! The C++ app used ofxSpout with an OpenGL context. This app has no GL
//! context (iced/wgpu), so the correct integration is **SpoutDX** (the
//! D3D11 flavor of the Spout 2.007 SDK), whose `ReceiveImage` can hand back
//! CPU pixels without any GL involvement.
//!
//! Integration status: the receiving backend is feature-gated (`spout`) and
//! requires a small C shim compiled against the Spout SDK:
//!
//! ```c
//! // spoutdx_shim.cpp — build with the SpoutDX sources (SpoutGL/SpoutDX)
//! extern "C" {
//!   void* spoutdx_new()                      { auto r = new spoutDX(); r->OpenDirectX11(); return r; }
//!   void  spoutdx_set_sender(void* r, const char* name) { ((spoutDX*)r)->SetReceiverName(name); }
//!   bool  spoutdx_receive_rgba(void* r, unsigned char* pixels, unsigned w, unsigned h)
//!                                            { return ((spoutDX*)r)->ReceiveImage(pixels, w, h, false, false); }
//!   unsigned spoutdx_sender_width(void* r)   { return ((spoutDX*)r)->GetSenderWidth(); }
//!   unsigned spoutdx_sender_height(void* r)  { return ((spoutDX*)r)->GetSenderHeight(); }
//!   void  spoutdx_release(void* r)           { ((spoutDX*)r)->ReleaseReceiver(); delete (spoutDX*)r; }
//! }
//! ```
//!
//! Compile that into `spoutdx_shim.dll` (linking the SpoutDX static lib) and
//! place it next to the executable; the `spout` feature then loads it at
//! runtime. Without the feature (or the DLL), the source renders black and
//! reports its status, so configs containing a Spout source still load.

use serde_json::{json, Value as Json};

use super::{deserialise_base, ImageSource, RenderContext, SourceBaseParams};
use crate::image::PixelsF32;

pub struct Spout {
    pub base: SourceBaseParams,
    /// Sender (channel) name; empty = active sender.
    pub sender_name: String,
    pub status: &'static str,
    #[cfg(feature = "spout")]
    backend: Option<backend::SpoutReceiver>,
}

impl Default for Spout {
    fn default() -> Self {
        Self {
            base: SourceBaseParams::default(),
            sender_name: String::new(),
            status: if cfg!(feature = "spout") {
                "spout: initializing"
            } else {
                "spout support not built (enable the `spout` feature)"
            },
            #[cfg(feature = "spout")]
            backend: None,
        }
    }
}

impl ImageSource for Spout {
    fn type_name(&self) -> &'static str {
        "Spout"
    }

    fn base(&self) -> &SourceBaseParams {
        &self.base
    }

    fn base_mut(&mut self) -> &mut SourceBaseParams {
        &mut self.base
    }

    #[allow(unused_variables)]
    fn render(&mut self, ctx: &RenderContext, out: &mut PixelsF32) {
        if out.width != ctx.width || out.height != ctx.height {
            *out = PixelsF32::new(ctx.width, ctx.height);
        }
        #[cfg(feature = "spout")]
        {
            if self.backend.is_none() {
                match backend::SpoutReceiver::new(&self.sender_name) {
                    Ok(receiver) => {
                        self.backend = Some(receiver);
                        self.status = "spout: connected";
                    }
                    Err(e) => {
                        self.status = e;
                    }
                }
            }
            if let Some(receiver) = &mut self.backend {
                if receiver.receive_into(out) {
                    return;
                }
                self.status = "spout: no sender";
            }
        }
        out.clear();
    }

    fn deserialise(&mut self, json: &Json) {
        deserialise_base(&mut self.base, json);
        if let Some(v) = json.get("senderName").and_then(|v| v.as_str()) {
            self.sender_name = v.to_string();
        }
    }

    fn serialise(&self) -> Json {
        json!({
            "type": "Spout",
            "visible": self.base.visible,
            "renderEnabled": self.base.render_enabled,
            "alpha": self.base.alpha,
            "style": self.base.style.as_str(),
            "senderName": self.sender_name,
            "status": self.status,
        })
    }
}

#[cfg(feature = "spout")]
mod backend {
    //! Runtime binding to `spoutdx_shim.dll` (see module docs).

    use crate::image::PixelsF32;

    pub struct SpoutReceiver {
        // handle + fn pointers, loaded from spoutdx_shim.dll
        instance: *mut std::ffi::c_void,
        receive: unsafe extern "C" fn(*mut std::ffi::c_void, *mut u8, u32, u32) -> bool,
        release: unsafe extern "C" fn(*mut std::ffi::c_void),
        rgba: Vec<u8>,
        _library: LoadedLibrary,
    }

    unsafe impl Send for SpoutReceiver {}

    struct LoadedLibrary(*mut std::ffi::c_void);
    unsafe impl Send for LoadedLibrary {}

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryA(name: *const u8) -> *mut std::ffi::c_void;
        fn GetProcAddress(module: *mut std::ffi::c_void, name: *const u8) -> *mut std::ffi::c_void;
    }

    impl SpoutReceiver {
        pub fn new(sender_name: &str) -> Result<Self, &'static str> {
            unsafe {
                let library = LoadLibraryA(b"spoutdx_shim.dll\0".as_ptr());
                if library.is_null() {
                    return Err("spout: spoutdx_shim.dll not found");
                }
                let get = |name: &'static [u8]| GetProcAddress(library, name.as_ptr());
                let new_fn = get(b"spoutdx_new\0");
                let set_sender = get(b"spoutdx_set_sender\0");
                let receive = get(b"spoutdx_receive_rgba\0");
                let release = get(b"spoutdx_release\0");
                if new_fn.is_null() || receive.is_null() || release.is_null() {
                    return Err("spout: shim exports missing");
                }
                let new_fn: unsafe extern "C" fn() -> *mut std::ffi::c_void =
                    std::mem::transmute(new_fn);
                let instance = new_fn();
                if instance.is_null() {
                    return Err("spout: init failed");
                }
                if !sender_name.is_empty() && !set_sender.is_null() {
                    let set_sender: unsafe extern "C" fn(*mut std::ffi::c_void, *const u8) =
                        std::mem::transmute(set_sender);
                    let mut name = sender_name.as_bytes().to_vec();
                    name.push(0);
                    set_sender(instance, name.as_ptr());
                }
                Ok(Self {
                    instance,
                    receive: std::mem::transmute(receive),
                    release: std::mem::transmute(release),
                    rgba: Vec::new(),
                    _library: LoadedLibrary(library),
                })
            }
        }

        pub fn receive_into(&mut self, out: &mut PixelsF32) -> bool {
            let (w, h) = (out.width, out.height);
            self.rgba.resize(w * h * 4, 0);
            let ok = unsafe { (self.receive)(self.instance, self.rgba.as_mut_ptr(), w as u32, h as u32) };
            if !ok {
                return false;
            }
            for (dst, src) in out.data.chunks_exact_mut(3).zip(self.rgba.chunks_exact(4)) {
                dst[0] = src[0] as f32 / 255.0;
                dst[1] = src[1] as f32 / 255.0;
                dst[2] = src[2] as f32 / 255.0;
            }
            true
        }
    }

    impl Drop for SpoutReceiver {
        fn drop(&mut self) {
            unsafe { (self.release)(self.instance) };
        }
    }
}
