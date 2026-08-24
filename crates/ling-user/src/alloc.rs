use core::ptr;

const CLASS_SIZES: [usize; 8] = [16, 32, 64, 128, 256, 512, 1024, 2048];
const HEAP_SIZE: usize = 4 * 1024 * 1024; // 4 MiB userland heap
const LARGE_FLAG: u64 = 1 << 63;

static mut HEAP_MEM: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut BUMP_PTR: usize = 0;
static mut FREE_HEAD: [u64; CLASS_SIZES.len()] = [0; CLASS_SIZES.len()];

fn class_for(needed: usize) -> Option<usize> {
    CLASS_SIZES.iter().position(|&sz| sz >= needed)
}

unsafe fn write_header(addr: u64, header: u64) {
    ptr::write_volatile(addr as *mut u64, header);
}

unsafe fn read_header(user_ptr: *mut u8) -> u64 {
    ptr::read_volatile((user_ptr as u64 - 8) as *const u64)
}

unsafe fn bump_alloc(bytes: usize) -> *mut u8 {
    let aligned = (bytes + 7) & !7;
    if BUMP_PTR + aligned > HEAP_SIZE {
        return ptr::null_mut();
    }
    let p = ((&raw mut HEAP_MEM) as *mut u8).add(BUMP_PTR);
    BUMP_PTR += aligned;
    p
}

pub fn alloc(size: usize) -> *mut u8 {
    let needed = size + 8;
    unsafe {
        if let Some(class) = class_for(needed) {
            let slot_size = CLASS_SIZES[class];
            if FREE_HEAD[class] != 0 {
                let addr = FREE_HEAD[class];
                FREE_HEAD[class] = ptr::read_volatile(addr as *const u64);
                write_header(addr, class as u64);
                return (addr + 8) as *mut u8;
            }
            let raw = bump_alloc(slot_size);
            if raw.is_null() {
                return ptr::null_mut();
            }
            let addr = raw as u64;
            write_header(addr, class as u64);
            return (addr + 8) as *mut u8;
        }

        let raw = bump_alloc(needed);
        if raw.is_null() {
            return ptr::null_mut();
        }
        let addr = raw as u64;
        write_header(addr, LARGE_FLAG | needed as u64);
        (addr + 8) as *mut u8
    }
}

pub fn free(user_ptr: *mut u8) {
    if user_ptr.is_null() {
        return;
    }
    unsafe {
        let header = read_header(user_ptr);
        let addr = user_ptr as u64 - 8;
        if header & LARGE_FLAG == 0 {
            let class = header as usize;
            if class < CLASS_SIZES.len() {
                write_header(addr, FREE_HEAD[class]);
                FREE_HEAD[class] = addr;
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_alloc(size: u64) -> u64 {
    let p = alloc(size as usize);
    if p.is_null() {
        ling_panic(0);
    }
    p as u64
}

#[no_mangle]
pub unsafe extern "C" fn ling_free(ptr: u64) -> u64 {
    free(ptr as *mut u8);
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_panic(msg_ptr: u64) -> ! {
    let msg: &[u8] = if msg_ptr != 0 {
        crate::strings::bytes_of_ptr(msg_ptr as *mut u8)
    } else {
        b"panic: memory exhausted or unhandled error\n"
    };
    crate::syscall::ling_sys_write(2, msg.as_ptr() as u64, msg.len() as u64);
    crate::syscall::ling_sys_exit(1);
    loop {}
}
