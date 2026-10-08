//! 最小 UEFI 引导接口。ExitBootServices 成功后不再调用固件显示、键盘、定时器等服务。
use core::{cell::UnsafeCell, ffi::c_void};
use mini_os_native::{
    graphics::Framebuffer,
    video::{DisplayMode, choose_mode},
};

#[repr(C)]
pub struct TableHeader {
    signature: u64,
    revision: u32,
    header_size: u32,
    crc32: u32,
    reserved: u32,
}
#[repr(C)]
pub struct SystemTable {
    header: TableHeader,
    vendor: *const u16,
    revision: u32,
    console_in_handle: *mut c_void,
    console_in: *mut c_void,
    console_out_handle: *mut c_void,
    console_out: *mut c_void,
    stderr_handle: *mut c_void,
    stderr: *mut c_void,
    runtime_services: *mut c_void,
    boot_services: *mut BootServices,
}
type Status = usize;
#[repr(C)]
struct BootServices {
    header: TableHeader,
    before_map: [usize; 4],
    get_memory_map:
        unsafe extern "efiapi" fn(*mut usize, *mut u8, *mut usize, *mut usize, *mut u32) -> Status,
    allocate_pool: usize,
    free_pool: unsafe extern "efiapi" fn(*mut c_void) -> Status,
    before_exit: [usize; 19],
    exit_boot_services: unsafe extern "efiapi" fn(*mut c_void, usize) -> Status,
    after_exit: [usize; 10],
    locate_protocol:
        unsafe extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> Status,
}
#[repr(C)]
struct Guid {
    a: u32,
    b: u16,
    c: u16,
    d: [u8; 8],
}
const GOP_GUID: Guid = Guid {
    a: 0x9042a9de,
    b: 0x23dc,
    c: 0x4a38,
    d: [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a],
};
#[repr(C)]
struct Gop {
    query_mode: unsafe extern "efiapi" fn(*mut Gop, u32, *mut usize, *mut *mut ModeInfo) -> Status,
    set_mode: unsafe extern "efiapi" fn(*mut Gop, u32) -> Status,
    blt: usize,
    mode: *mut GopMode,
}
#[repr(C)]
struct GopMode {
    max_mode: u32,
    mode: u32,
    info: *mut ModeInfo,
    info_size: usize,
    framebuffer: u64,
    framebuffer_size: usize,
}
#[repr(C)]
struct ModeInfo {
    version: u32,
    width: u32,
    height: u32,
    pixel_format: u32,
    bitmask: [u32; 4],
    stride: u32,
}
#[repr(C, align(16))]
struct MapBuffer(UnsafeCell<[u8; 65536]>);
// 引导入口独占访问；退出固件后仅在内核初始化期间读取。
unsafe impl Sync for MapBuffer {}
static MEMORY_MAP: MapBuffer = MapBuffer(UnsafeCell::new([0; 65536]));

pub struct BootInfo {
    pub framebuffer: Framebuffer,
    pub map: *const u8,
    pub map_size: usize,
    pub descriptor_size: usize,
}

/// # Safety
/// image 和 table 必须来自当前 UEFI 固件入口；成功后不能再调用 Boot Services。
pub unsafe fn takeover(
    image: *mut c_void,
    table: *mut SystemTable,
) -> Result<BootInfo, &'static str> {
    unsafe {
        if table.is_null() || (*table).boot_services.is_null() {
            return Err("Invalid UEFI system table");
        }
        let services = &*(*table).boot_services;
        let mut protocol = core::ptr::null_mut();
        if (services.locate_protocol)(&GOP_GUID, core::ptr::null_mut(), &mut protocol) != 0
            || protocol.is_null()
        {
            return Err("Graphics Output Protocol missing");
        }
        let gop = protocol.cast::<Gop>();
        if (*gop).mode.is_null() {
            return Err("GOP mode missing");
        }
        let current = (*(*gop).mode).mode;
        let mut modes = [None; 256];
        for index in 0..(*(*gop).mode).max_mode.min(256) {
            let mut size = 0;
            let mut info = core::ptr::null_mut();
            if ((*gop).query_mode)(gop, index, &mut size, &mut info) == 0 && !info.is_null() {
                if size >= core::mem::size_of::<ModeInfo>() {
                    modes[index as usize] = Some((
                        index,
                        DisplayMode {
                            width: (*info).width,
                            height: (*info).height,
                            stride: (*info).stride,
                            pixel_format: (*info).pixel_format,
                        },
                    ));
                }
                (services.free_pool)(info.cast());
            }
        }
        let selected = choose_mode(modes.into_iter().flatten(), current)
            .ok_or("No compatible framebuffer mode")?;
        if ((*gop).set_mode)(gop, selected) != 0 {
            return Err("Cannot set graphics mode");
        }
        let mode = &*(*gop).mode;
        if mode.info.is_null() || mode.info_size < core::mem::size_of::<ModeInfo>() {
            return Err("GOP mode info missing");
        }
        let info = &*mode.info;
        if info.pixel_format >= 2
            || info.width < 800
            || info.height < 600
            || info.stride < info.width
        {
            return Err("Unsupported framebuffer format or resolution");
        }
        let required = (info.stride as usize)
            .checked_mul(info.height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or("Framebuffer size overflow")?;
        if mode.framebuffer == 0 || required > mode.framebuffer_size {
            return Err("Invalid framebuffer bounds");
        }
        let framebuffer = Framebuffer::new(
            mode.framebuffer as usize,
            info.width as usize,
            info.height as usize,
            info.stride as usize,
            info.pixel_format == 0,
        );
        let map = MEMORY_MAP.0.get().cast::<u8>();
        for _ in 0..4 {
            let mut map_size = 65536;
            let mut key = 0;
            let mut descriptor_size = 0;
            let mut version = 0;
            if (services.get_memory_map)(
                &mut map_size,
                map,
                &mut key,
                &mut descriptor_size,
                &mut version,
            ) != 0
            {
                return Err("GetMemoryMap failed");
            }
            if descriptor_size < 40 || map_size > 65536 || !map_size.is_multiple_of(descriptor_size)
            {
                return Err("Invalid memory descriptors");
            }
            // 此处与 ExitBootServices 之间没有分配或其他固件调用。
            if (services.exit_boot_services)(image, key) == 0 {
                core::arch::asm!("cli", options(nomem, nostack));
                return Ok(BootInfo {
                    framebuffer,
                    map,
                    map_size,
                    descriptor_size,
                });
            }
        }
        Err("ExitBootServices failed after fresh memory-map retries")
    }
}
