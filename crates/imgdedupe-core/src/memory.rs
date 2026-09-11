//! How much memory the machine has to spare.

/// Memory available to the machine right now, or nothing when it will not say.
pub fn available_bytes() -> Option<u64> {
    imp::available_bytes()
}

#[cfg(target_os = "linux")]
mod imp {
    /// The `MemAvailable:` line of `/proc/meminfo`, which is in kibibytes.
    pub fn available_bytes() -> Option<u64> {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = meminfo
            .lines()
            .find(|line| line.starts_with("MemAvailable:"))?;
        let kibibytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        kibibytes.checked_mul(1024)
    }
}

#[cfg(windows)]
mod imp {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    pub fn available_bytes() -> Option<u64> {
        let mut status = MemoryStatusEx {
            length: std::mem::size_of::<MemoryStatusEx>() as u32,
            memory_load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page_file: 0,
            avail_page_file: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended_virtual: 0,
        };
        // Safety: `status` is a MEMORYSTATUSEX with its `length` set, which is
        // the whole of what the call requires.
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
        if ok == 0 {
            return None;
        }
        Some(status.avail_phys)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    /// `HOST_VM_INFO64`.
    const VM_INFO64: i32 = 4;

    /// `struct vm_statistics64`, in the order the kernel fills it.
    #[repr(C)]
    #[derive(Default)]
    struct VmStatistics64 {
        free_count: u32,
        active_count: u32,
        inactive_count: u32,
        wire_count: u32,
        zero_fill_count: u64,
        reactivations: u64,
        pageins: u64,
        pageouts: u64,
        faults: u64,
        cow_faults: u64,
        lookups: u64,
        hits: u64,
        purges: u64,
        purgeable_count: u32,
        speculative_count: u32,
        decompressions: u64,
        compressions: u64,
        swapins: u64,
        swapouts: u64,
        compressor_page_count: u32,
        throttled_count: u32,
        external_page_count: u32,
        internal_page_count: u32,
        total_uncompressed_pages_in_compressor: u64,
    }

    extern "C" {
        fn mach_host_self() -> u32;
        fn host_page_size(host: u32, out: *mut usize) -> i32;
        fn host_statistics64(host: u32, flavor: i32, out: *mut u32, count: *mut u32) -> i32;
    }

    pub fn available_bytes() -> Option<u64> {
        let host = unsafe { mach_host_self() };

        let mut page_size: usize = 0;
        // Safety: `page_size` is one `vm_size_t` for the call to write.
        if unsafe { host_page_size(host, &mut page_size) } != 0 {
            return None;
        }

        let mut stats = VmStatistics64::default();
        let mut count = (std::mem::size_of::<VmStatistics64>() / std::mem::size_of::<u32>()) as u32;
        // Safety: `stats` is a `vm_statistics64` and `count` says how many
        // 32 bit words of it there are, which is what the call is given.
        let ok = unsafe {
            host_statistics64(
                host,
                VM_INFO64,
                (&mut stats as *mut VmStatistics64).cast(),
                &mut count,
            )
        };
        if ok != 0 {
            return None;
        }

        let pages = u64::from(stats.free_count)
            + u64::from(stats.inactive_count)
            + u64::from(stats.purgeable_count);
        pages.checked_mul(page_size as u64)
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod imp {
    pub fn available_bytes() -> Option<u64> {
        None
    }
}
