#![allow(non_snake_case)]
#![allow(unpredictable_function_pointer_comparisons)]
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]
#![allow(clippy::missing_safety_doc)]
#![doc = include_str!("../README.md")]

// ferros (this fork): the C++ runtime shims, POSIX and stdio openh264 calls (ferros_rt feature `cxx`), linked into every process that links openh264.
extern crate ferros_rt as _;

mod error;

/// Generated bindings for OpenH264.
mod generated {
    pub mod consts;
    #[cfg(feature = "source")]
    pub mod fns_source;
    pub mod types;
}

pub use self::generated::consts::*;
pub use self::generated::types::*;
pub use error::Error;
use std::os::raw::{c_int, c_long};

/// Abstraction over the `source` API (this fork offers no `libloading`).
#[rustfmt::skip]
#[allow(clippy::missing_safety_doc)]
pub trait API {
    unsafe fn WelsCreateSVCEncoder(&self, ppEncoder: *mut *mut ISVCEncoder) -> ::std::os::raw::c_int;
    unsafe fn WelsDestroySVCEncoder(&self, pEncoder: *mut ISVCEncoder);
    unsafe fn WelsGetDecoderCapability(&self, pDecCapability: *mut SDecoderCapability) -> ::std::os::raw::c_int;
    unsafe fn WelsCreateDecoder(&self, ppDecoder: *mut *mut ISVCDecoder) -> ::std::os::raw::c_long;
    unsafe fn WelsDestroyDecoder(&self, pDecoder: *mut ISVCDecoder);
    unsafe fn WelsGetCodecVersion(&self) -> OpenH264Version;
    unsafe fn WelsGetCodecVersionEx(&self, pVersion: *mut OpenH264Version);
}


/// API surface using built-in source.
///
/// This API surface should _just work_ once compiled. Depending on your commercial, legal and geographic situation, and the H.264 features you use,
/// this might or might not come with an elevated patent risk.
#[cfg(feature = "source")]
pub mod source {
    use crate::{ISVCDecoder, ISVCEncoder, OpenH264Version, SDecoderCapability};
    use std::os::raw::{c_int, c_long};

    #[derive(Debug, Default)]
    pub struct APILoader;

    #[rustfmt::skip]
    #[allow(clippy::missing_safety_doc)]
    impl APILoader {
        pub const fn new() -> Self { Self }
        pub unsafe fn WelsCreateSVCEncoder(ppEncoder: *mut *mut ISVCEncoder) -> ::std::os::raw::c_int { unsafe { crate::generated::fns_source::WelsCreateSVCEncoder(ppEncoder) }}
        pub unsafe fn WelsDestroySVCEncoder(pEncoder: *mut ISVCEncoder) { unsafe { crate::generated::fns_source::WelsDestroySVCEncoder(pEncoder) }}
        pub unsafe fn WelsGetDecoderCapability(pDecCapability: *mut SDecoderCapability) -> ::std::os::raw::c_int { unsafe { crate::generated::fns_source::WelsGetDecoderCapability(pDecCapability) }}
        pub unsafe fn WelsCreateDecoder(ppDecoder: *mut *mut ISVCDecoder) -> ::std::os::raw::c_long { unsafe { crate::generated::fns_source::WelsCreateDecoder(ppDecoder) }}
        pub unsafe fn WelsDestroyDecoder(pDecoder: *mut ISVCDecoder) { unsafe { crate::generated::fns_source::WelsDestroyDecoder(pDecoder) }}
        pub unsafe fn WelsGetCodecVersion() -> OpenH264Version { unsafe { crate::generated::fns_source::WelsGetCodecVersion() }}
        pub unsafe fn WelsGetCodecVersionEx(pVersion: *mut OpenH264Version) { unsafe { crate::generated::fns_source::WelsGetCodecVersionEx(pVersion) }}
    }

    #[rustfmt::skip]
    #[allow(clippy::missing_safety_doc)]
    impl super::API for APILoader {
        unsafe fn WelsCreateSVCEncoder(&self, ppEncoder: *mut *mut ISVCEncoder) -> c_int { unsafe { APILoader::WelsCreateSVCEncoder(ppEncoder) }}
        unsafe fn WelsDestroySVCEncoder(&self, pEncoder: *mut ISVCEncoder) { unsafe { APILoader::WelsDestroySVCEncoder(pEncoder) }}
        unsafe fn WelsGetDecoderCapability(&self, pDecCapability: *mut SDecoderCapability) -> c_int { unsafe { APILoader::WelsGetDecoderCapability(pDecCapability) }}
        unsafe fn WelsCreateDecoder(&self, ppDecoder: *mut *mut ISVCDecoder) -> c_long { unsafe { APILoader::WelsCreateDecoder(ppDecoder) }}
        unsafe fn WelsDestroyDecoder(&self, pDecoder: *mut ISVCDecoder) { unsafe { APILoader::WelsDestroyDecoder(pDecoder) }}
        unsafe fn WelsGetCodecVersion(&self) -> OpenH264Version { unsafe { APILoader::WelsGetCodecVersion() }}
        unsafe fn WelsGetCodecVersionEx(&self, pVersion: *mut OpenH264Version) { unsafe { APILoader::WelsGetCodecVersionEx(pVersion) }}
    }
}

/// Convenience wrapper around the `source` API surface.
///
/// This type mainly exists to prevent infecting the rest of the OpenH264 crate with generics. The dispatch overhead in contrast to H.264 computation is absolutely negligible.
#[allow(clippy::large_enum_variant)]
pub enum DynamicAPI {
    #[cfg(feature = "source")]
    Source(source::APILoader),
}

impl DynamicAPI {
    /// Creates an OpenH264 API using the built-in source if available.
    #[cfg(feature = "source")]
    pub const fn from_source() -> Self {
        let api = crate::source::APILoader;
        Self::Source(api)
    }
}

#[allow(unreachable_patterns)]
#[allow(unused)]
impl API for DynamicAPI {
    unsafe fn WelsCreateSVCEncoder(&self, ppEncoder: *mut *mut ISVCEncoder) -> c_int {
        unsafe {
            match self {
                #[cfg(feature = "source")]
                DynamicAPI::Source(api) => api.WelsCreateSVCEncoder(ppEncoder),
                _ => panic!("No API enabled"),
            }
        }
    }

    unsafe fn WelsDestroySVCEncoder(&self, pEncoder: *mut ISVCEncoder) {
        unsafe {
            match self {
                #[cfg(feature = "source")]
                DynamicAPI::Source(api) => api.WelsDestroySVCEncoder(pEncoder),
                _ => panic!("No API enabled"),
            }
        }
    }

    unsafe fn WelsGetDecoderCapability(&self, pDecCapability: *mut SDecoderCapability) -> c_int {
        unsafe {
            match self {
                #[cfg(feature = "source")]
                DynamicAPI::Source(api) => api.WelsGetDecoderCapability(pDecCapability),
                _ => panic!("No API enabled"),
            }
        }
    }

    unsafe fn WelsCreateDecoder(&self, ppDecoder: *mut *mut ISVCDecoder) -> c_long {
        unsafe {
            match self {
                #[cfg(feature = "source")]
                DynamicAPI::Source(api) => api.WelsCreateDecoder(ppDecoder),
                _ => panic!("No API enabled"),
            }
        }
    }

    unsafe fn WelsDestroyDecoder(&self, pDecoder: *mut ISVCDecoder) {
        unsafe {
            match self {
                #[cfg(feature = "source")]
                DynamicAPI::Source(api) => api.WelsDestroyDecoder(pDecoder),
                _ => panic!("No API enabled"),
            }
        }
    }

    unsafe fn WelsGetCodecVersion(&self) -> OpenH264Version {
        unsafe {
            match self {
                #[cfg(feature = "source")]
                DynamicAPI::Source(api) => api.WelsGetCodecVersion(),
                _ => panic!("No API enabled"),
            }
        }
    }

    unsafe fn WelsGetCodecVersionEx(&self, pVersion: *mut OpenH264Version) {
        unsafe {
            match self {
                #[cfg(feature = "source")]
                DynamicAPI::Source(api) => api.WelsGetCodecVersionEx(pVersion),
                _ => panic!("No API enabled"),
            }
        }
    }
}
