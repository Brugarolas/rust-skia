use super::{Device, GetProc, GetProcOf, Instance, PhysicalDevice, Queue, Version};
use crate::{gpu, prelude::*};
use ffi::CString;
use raw::c_char;
use skia_bindings::{self as sb, GrVkExtensionFlags, GrVkFeatureFlags};
use std::{cell::RefCell, ffi, fmt, mem, ops::Deref, os::raw, ptr};

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ExtensionFlags : u32 {
        const EXT_DEBUG_REPORT = sb::GrVkExtensionFlags_kEXT_debug_report_GrVkExtensionFlag as _;
        const NV_GLSL_SHADER = sb::GrVkExtensionFlags_kNV_glsl_shader_GrVkExtensionFlag as _;
        const KHR_SURFACE = sb::GrVkExtensionFlags_kKHR_surface_GrVkExtensionFlag as _;
        const KHR_SWAPCHAIN = sb::GrVkExtensionFlags_kKHR_swapchain_GrVkExtensionFlag as _;
        const KHR_WIN32_SURFACE = sb::GrVkExtensionFlags_kKHR_win32_surface_GrVkExtensionFlag as _;
        const KHR_ANDROID_SURFACE = sb::GrVkExtensionFlags_kKHR_android_surface_GrVkExtensionFlag as _;
        const KHR_XCB_SURFACE = sb::GrVkExtensionFlags_kKHR_xcb_surface_GrVkExtensionFlag as _;
    }
}

native_transmutable!(GrVkExtensionFlags, ExtensionFlags, extension_flags_layout);

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct FeatureFlags: u32 {
        const GEOMETRY_SHADER = sb::GrVkFeatureFlags_kGeometryShader_GrVkFeatureFlag as _;
        const DUAL_SRC_BLEND = sb::GrVkFeatureFlags_kDualSrcBlend_GrVkFeatureFlag as _;
        const SAMPLE_RATE_SHADING = sb::GrVkFeatureFlags_kSampleRateShading_GrVkFeatureFlag as _;
    }
}

native_transmutable!(GrVkFeatureFlags, FeatureFlags, feature_flags_layout);

// Note: the GrBackendContext's layout generated by bindgen does not match in size,
// so we do need to use a pointer here for now.
pub struct BackendContext<'a> {
    pub(crate) native: ptr::NonNull<ffi::c_void>,
    get_proc: &'a dyn GetProc,
}

impl<'a> Drop for BackendContext<'a> {
    fn drop(&mut self) {
        unsafe { sb::C_GrVkBackendContext_delete(self.native.as_ptr()) }
    }
}

impl fmt::Debug for BackendContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackendContext")
            .field("native", &self.native)
            .finish()
    }
}

// TODO: add some accessor functions to the public fields.
// TODO: may support Clone (note the original structure holds a smartpointer!)
// TODO: think about making this safe in respect to the lifetime of the handles
//       it refers to.
impl BackendContext<'_> {
    /// # Safety
    /// `instance`, `physical_device`, `device`, and `queue` must outlive the `BackendContext`
    /// returned.
    pub unsafe fn new(
        instance: Instance,
        physical_device: PhysicalDevice,
        device: Device,
        (queue, queue_index): (Queue, usize),
        get_proc: &impl GetProc,
    ) -> BackendContext {
        Self::new_with_extensions(
            instance,
            physical_device,
            device,
            (queue, queue_index),
            get_proc,
            &[],
            &[],
        )
    }

    /// # Safety
    /// `instance`, `physical_device`, `device`, and `queue` must outlive the `BackendContext`
    /// returned.
    pub unsafe fn new_with_extensions<'a>(
        instance: Instance,
        physical_device: PhysicalDevice,
        device: Device,
        (queue, queue_index): (Queue, usize),
        get_proc: &'a impl GetProc,
        instance_extensions: &[&str],
        device_extensions: &[&str],
    ) -> BackendContext<'a> {
        // pin the extensions string in memory and provide pointers to the NewWithExtension function,
        // but there is no need to retain them, because because the implementations copies these strings, too.
        let instance_extensions: Vec<CString> = instance_extensions
            .iter()
            .map(|str| CString::new(*str).unwrap())
            .collect();
        let instance_extensions: Vec<*const c_char> =
            instance_extensions.iter().map(|cs| cs.as_ptr()).collect();
        let device_extensions: Vec<CString> = device_extensions
            .iter()
            .map(|str| CString::new(*str).unwrap())
            .collect();
        let device_extensions: Vec<*const c_char> =
            device_extensions.iter().map(|cs| cs.as_ptr()).collect();

        let resolver = Self::begin_resolving_proc(get_proc);
        let native = sb::C_GrVkBackendContext_new(
            instance as _,
            physical_device as _,
            device as _,
            queue as _,
            queue_index.try_into().unwrap(),
            Some(global_get_proc),
            instance_extensions.as_ptr(),
            instance_extensions.len(),
            device_extensions.as_ptr(),
            device_extensions.len(),
        );
        drop(resolver);
        BackendContext {
            native: ptr::NonNull::new(native).unwrap(),
            get_proc,
        }
    }

    pub fn set_protected_context(&mut self, protected_context: gpu::Protected) -> &mut Self {
        unsafe {
            sb::C_GrVkBackendContext_setProtectedContext(
                self.native.as_ptr() as _,
                protected_context,
            )
        }
        self
    }

    pub fn set_max_api_version(&mut self, version: impl Into<Version>) -> &mut Self {
        unsafe {
            sb::C_GrVkBackendContext_setMaxAPIVersion(
                self.native.as_ptr() as _,
                *version.into().deref(),
            )
        }
        self
    }

    pub(crate) unsafe fn begin_resolving(&self) -> impl Drop {
        Self::begin_resolving_proc(self.get_proc)
    }

    // The idea here is to set up a thread local variable with the GetProc function trait
    // and reroute queries to global_get_proc as long the caller does not invoke the Drop
    // impl trait that is returned.
    // This is an attempt to support Rust Closures / Functions that resolve function pointers instead
    // of relying on a global extern "C" function.
    // TODO: This is a mess, highly unsafe, and needs to be simplified / rewritten
    //       by someone who understands Rust better.
    unsafe fn begin_resolving_proc(get_proc_trait_object: &dyn GetProc) -> impl Drop {
        THREAD_LOCAL_GET_PROC
            .with(|get_proc| *get_proc.borrow_mut() = Some(mem::transmute(get_proc_trait_object)));

        EndResolving {}
    }
}

struct EndResolving {}

impl Drop for EndResolving {
    fn drop(&mut self) {
        THREAD_LOCAL_GET_PROC.with(|get_proc| *get_proc.borrow_mut() = None)
    }
}

thread_local! {
    static THREAD_LOCAL_GET_PROC: RefCell<Option<TraitObject>> = const { RefCell::new(None) };
}

// https://doc.rust-lang.org/1.19.0/std/raw/struct.TraitObject.html
#[repr(C)]
// Copy & Clone are required for the *get_proc.borrow() below. And std::raw::TraitObject
// can not be used, because it's unstable (last checked 1.36).
#[derive(Copy, Clone)]
struct TraitObject {
    pub data: *mut (),
    pub vtable: *mut (),
}

// The global resolvement function passed to Skia.
unsafe extern "C" fn global_get_proc(
    name: *const raw::c_char,
    instance: Instance,
    device: Device,
) -> *const raw::c_void {
    THREAD_LOCAL_GET_PROC.with(|get_proc| {
        match *get_proc.borrow() {
            Some(get_proc) => {
                let get_proc_trait_object: &dyn GetProc = mem::transmute(get_proc);
                if !device.is_null() {
                    get_proc_trait_object(GetProcOf::Device(device, name))
                } else {
                    // note: instance may be null here!
                    get_proc_trait_object(GetProcOf::Instance(instance, name))
                }
            }
            None => panic!("Vulkan GetProc called outside of a thread local resolvement context."),
        }
    })
}
