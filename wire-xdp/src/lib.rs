use std::ptr;
use std::sync::atomic::{fence, Ordering};
use wire_core::probes::{self, StageId};
use wire_simd::ParsedL4;

pub const NUM_FRAMES: usize = 4096;
pub const RX_FRAMES: usize = 2048;
pub const TX_FRAMES: usize = 2048;
pub const FRAME_SIZE: usize = 2048;
pub const UMEM_SIZE: usize = NUM_FRAMES * FRAME_SIZE;
pub const BATCH_SIZE: usize = 64;

const SOL_XDP: libc::c_int = 283;
const XDP_UMEM_REG: libc::c_int = 3;
const XDP_UMEM_FILL_RING: libc::c_int = 5;
const XDP_UMEM_COMPLETION_RING: libc::c_int = 6;
const XDP_RX_RING: libc::c_int = 1;
const XDP_TX_RING: libc::c_int = 2;
const XDP_MMAP_OFFSETS: libc::c_int = 1;
const XDP_COPY: u16 = 1 << 1;

const BPF_MAP_CREATE: i32 = 0;
const BPF_MAP_UPDATE_ELEM: i32 = 2;
const BPF_PROG_LOAD: i32 = 5;
const BPF_MAP_TYPE_XSKMAP: u32 = 17;
const BPF_MAP_TYPE_HASH: u32 = 1;
const BPF_PROG_TYPE_XDP: u32 = 6;
const MAP_HUGE_2MB: libc::c_int = 21 << 26;

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct XdpUmemReg {
    pub addr: u64,
    pub len: u64,
    pub chunk_size: u32,
    pub headroom: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct XdpRingOffset {
    pub producer: u64,
    pub consumer: u64,
    pub desc: u64,
    pub flags: u64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct XdpMmapOffsets {
    pub rx: XdpRingOffset,
    pub tx: XdpRingOffset,
    pub fr: XdpRingOffset,
    pub cr: XdpRingOffset,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SockAddrXdp {
    pub sxdp_family: u16,
    pub sxdp_flags: u16,
    pub sxdp_ifindex: u32,
    pub sxdp_queue_id: u32,
    pub sxdp_shared_umem_fd: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct XdpDesc {
    pub addr: u64,
    pub len: u32,
    pub options: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BpfMapAttr {
    map_type: u32,
    key_size: u32,
    value_size: u32,
    max_entries: u32,
    map_flags: u32,
    inner_map_fd: u32,
    numa_node: u32,
    map_name: [u8; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BpfProgAttr {
    prog_type: u32,
    insn_cnt: u32,
    insns: u64,
    license: u64,
    log_level: u32,
    log_size: u32,
    log_buf: u64,
    kern_version: u32,
    prog_flags: u32,
    prog_name: [u8; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BpfMapOpAttr {
    map_fd: u32,
    key: u64,
    value: u64,
    flags: u64,
}

pub fn pin_thread_to_core(core_id: usize) -> anyhow::Result<()> {
    // SAFETY: Binding execution thread to dedicated CPU core via sched_setaffinity.
    unsafe {
        let mut cpuset: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(core_id, &mut cpuset);
        let res = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &cpuset);
        if res != 0 {
            return Err(anyhow::anyhow!("Affinity error: {}", std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

fn bpf_syscall(cmd: i32, attr: *const u8, size: usize) -> i32 {
    // SAFETY: Direct BPF kernel system call invocation.
    unsafe { libc::syscall(libc::SYS_bpf, cmd, attr, size) as i32 }
}

pub struct XdpBpfModule {
    pub xsks_map_fd: i32,
    pub allowed_ports_map_fd: i32,
    pub prog_fd: i32,
}

impl XdpBpfModule {
    pub fn load_and_attach(ifname: &str, elf_bytes: &[u8]) -> anyhow::Result<Self> {
        let (xsks_map_fd, allowed_ports_map_fd, instructions) = Self::parse_elf_and_relocate(elf_bytes)?;

        let license = b"GPL\0";
        let mut log_buf = vec![0u8; 65536];
        let mut prog_attr = BpfProgAttr {
            prog_type: BPF_PROG_TYPE_XDP,
            insn_cnt: instructions.len() as u32,
            insns: instructions.as_ptr() as u64,
            license: license.as_ptr() as u64,
            log_level: 1,
            log_size: log_buf.len() as u32,
            log_buf: log_buf.as_mut_ptr() as u64,
            kern_version: 0,
            prog_flags: 0,
            prog_name: [0; 16],
        };
        prog_attr.prog_name[..11].copy_from_slice(b"xdp_red_xsk");

        let prog_fd = bpf_syscall(BPF_PROG_LOAD, &prog_attr as *const _ as *const u8, std::mem::size_of::<BpfProgAttr>());
        if prog_fd < 0 {
            // SAFETY: Cleaning map file descriptors on loader error state.
            unsafe {
                libc::close(xsks_map_fd);
                libc::close(allowed_ports_map_fd);
            }
            return Err(anyhow::anyhow!("BPF Program load error: {}", std::io::Error::last_os_error()));
        }

        // SAFETY: Interface name to interface index libc lookup.
        let ifindex = unsafe {
            let cname = std::ffi::CString::new(ifname)?;
            libc::if_nametoindex(cname.as_ptr())
        };
        if ifindex == 0 {
            return Err(anyhow::anyhow!("Interface lookup failed: {}", ifname));
        }

        // SAFETY: Direct Netlink configuration payload submission.
        let nl_fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW, libc::NETLINK_ROUTE) };
        if nl_fd >= 0 {
            #[repr(C)]
            struct NlMsg {
                hdr: libc::nlmsghdr,
                ifi: libc::ifinfomsg,
                attr_hdr: libc::nlattr,
                xdp_hdr: libc::nlattr,
                fd_attr: libc::nlattr,
                fd_val: i32,
            }

            let mut ifi: libc::ifinfomsg = unsafe { std::mem::zeroed() };
            ifi.ifi_index = ifindex as i32;

            let msg = NlMsg {
                hdr: libc::nlmsghdr {
                    nlmsg_len: std::mem::size_of::<NlMsg>() as u32,
                    nlmsg_type: 16,
                    nlmsg_flags: (0x01 | 0x100 | 0x400) as u16,
                    nlmsg_seq: 1,
                    nlmsg_pid: 0,
                },
                ifi,
                attr_hdr: libc::nlattr {
                    nla_len: (std::mem::size_of::<libc::nlattr>() * 2 + std::mem::size_of::<i32>()) as u16,
                    nla_type: 43,
                },
                xdp_hdr: libc::nlattr {
                    nla_len: (std::mem::size_of::<libc::nlattr>() + std::mem::size_of::<i32>()) as u16,
                    nla_type: 1,
                },
                fd_attr: libc::nlattr {
                    nla_len: 8,
                    nla_type: 3,
                },
                fd_val: prog_fd,
            };
            unsafe {
                libc::send(nl_fd, &msg as *const _ as *const libc::c_void, std::mem::size_of::<NlMsg>(), 0);
                libc::close(nl_fd);
            }
        }

        Ok(Self { xsks_map_fd, allowed_ports_map_fd, prog_fd })
    }

    fn parse_elf_and_relocate(elf: &[u8]) -> anyhow::Result<(i32, i32, Vec<u64>)> {
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into()?) as usize;
        let e_shentsize = u16::from_le_bytes(elf[58..60].try_into()?) as usize;
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into()?) as usize;
        let e_shstrndx = u16::from_le_bytes(elf[62..64].try_into()?) as usize;

        let mut sh_headers = Vec::new();
        for i in 0..e_shnum {
            let offset = e_shoff + i * e_shentsize;
            let sh_name = u32::from_le_bytes(elf[offset..offset+4].try_into()?) as usize;
            let sh_type = u32::from_le_bytes(elf[offset+4..offset+8].try_into()?);
            let sh_offset = u64::from_le_bytes(elf[offset+24..offset+32].try_into()?) as usize;
            let sh_size = u64::from_le_bytes(elf[offset+32..offset+40].try_into()?) as usize;
            sh_headers.push((sh_name, sh_type, sh_offset, sh_size));
        }

        let str_sec_offset = sh_headers[e_shstrndx].2;
        let get_string = |name_offset: usize| -> String {
            let mut end = name_offset;
            while elf[str_sec_offset + end] != 0 {
                end += 1;
            }
            String::from_utf8_lossy(&elf[str_sec_offset + name_offset..str_sec_offset + end]).into_owned()
        };

        let mut xdp_section_offset = 0;
        let mut xdp_section_size = 0;
        let mut rel_section_offset = 0;
        let mut rel_section_size = 0;
        let mut symtab_offset = 0;
        let mut strtab_offset = 0;

        for (sh_name, sh_type, sh_offset, sh_size) in &sh_headers {
            let name = get_string(*sh_name);
            if name == "xdp" {
                xdp_section_offset = *sh_offset;
                xdp_section_size = *sh_size;
            } else if name == ".relxdp" {
                rel_section_offset = *sh_offset;
                rel_section_size = *sh_size;
            } else if *sh_type == 2 {
                symtab_offset = *sh_offset;
            } else if name == ".strtab" {
                strtab_offset = *sh_offset;
            }
        }

        let mut xsks_attr = BpfMapAttr {
            map_type: BPF_MAP_TYPE_XSKMAP,
            key_size: 4,
            value_size: 4,
            max_entries: 64,
            map_flags: 0,
            inner_map_fd: 0,
            numa_node: 0,
            map_name: [0; 16],
        };
        xsks_attr.map_name[..8].copy_from_slice(b"xsks_map");
        let xsks_map_fd = bpf_syscall(BPF_MAP_CREATE, &xsks_attr as *const _ as *const u8, std::mem::size_of::<BpfMapAttr>());
        if xsks_map_fd < 0 {
            return Err(anyhow::anyhow!("Failed to create xskmap: {}", std::io::Error::last_os_error()));
        }

        let mut ports_attr = BpfMapAttr {
            map_type: BPF_MAP_TYPE_HASH,
            key_size: 2,
            value_size: 1,
            max_entries: 256,
            map_flags: 0,
            inner_map_fd: 0,
            numa_node: 0,
            map_name: [0; 16],
        };
        ports_attr.map_name[..13].copy_from_slice(b"allowed_ports");
        let allowed_ports_fd = bpf_syscall(BPF_MAP_CREATE, &ports_attr as *const _ as *const u8, std::mem::size_of::<BpfMapAttr>());
        if allowed_ports_fd < 0 {
            // SAFETY: Clean up previously created map file descriptors.
            unsafe { libc::close(xsks_map_fd); }
            return Err(anyhow::anyhow!("Failed to create allowed_ports map: {}", std::io::Error::last_os_error()));
        }

        let mut raw_insns = vec![0u64; xdp_section_size / 8];
        // SAFETY: Copying ELF bytecode into instruction buffer.
        unsafe {
            ptr::copy_nonoverlapping(
                elf.as_ptr().add(xdp_section_offset),
                raw_insns.as_mut_ptr() as *mut u8,
                xdp_section_size,
            );
        }

        let get_symbol_name = |sym_idx: usize| -> String {
            let sym_offset = symtab_offset + sym_idx * 24;
            let st_name = u32::from_le_bytes(elf[sym_offset..sym_offset+4].try_into().unwrap()) as usize;
            let mut end = st_name;
            while elf[strtab_offset + end] != 0 {
                end += 1;
            }
            String::from_utf8_lossy(&elf[strtab_offset + st_name..strtab_offset + end]).into_owned()
        };

        let num_rel = rel_section_size / 16;
        for i in 0..num_rel {
            let rel_offset = rel_section_offset + i * 16;
            let r_offset = u64::from_le_bytes(elf[rel_offset..rel_offset+8].try_into()?) as usize;
            let r_info = u64::from_le_bytes(elf[rel_offset+8..rel_offset+16].try_into()?);
            let sym_idx = (r_info >> 32) as usize;

            let sym_name = get_symbol_name(sym_idx);
            let fd_val = if sym_name == "xsks_map" {
                xsks_map_fd
            } else if sym_name == "allowed_ports" {
                allowed_ports_fd
            } else {
                continue;
            };

            let insn_idx = r_offset / 8;
            let insn = raw_insns[insn_idx];
            let opcode = insn & 0xFF;
            if opcode == 0x18 {
                let masked_insn = insn & !(0xF00);
                raw_insns[insn_idx] = masked_insn | (1 << 8);

                let cleared_imm = raw_insns[insn_idx] & !(0xFFFFFFFF << 32);
                raw_insns[insn_idx] = cleared_imm | ((fd_val as u64) << 32);
            }
        }

        Ok((xsks_map_fd, allowed_ports_fd, raw_insns))
    }

    pub fn update_xsk_map(&self, queue_id: u32, xsk_fd: i32) -> anyhow::Result<()> {
        let attr = BpfMapOpAttr {
            map_fd: self.xsks_map_fd as u32,
            key: &queue_id as *const _ as u64,
            value: &xsk_fd as *const _ as u64,
            flags: 0,
        };
        let res = bpf_syscall(BPF_MAP_UPDATE_ELEM, &attr as *const _ as *const u8, std::mem::size_of::<BpfMapOpAttr>());
        if res < 0 {
            return Err(anyhow::anyhow!("xsks_map update failed: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    pub fn register_port(&self, port: u16) -> anyhow::Result<()> {
        let dport = port.to_be();
        let val: u8 = 1;
        let attr = BpfMapOpAttr {
            map_fd: self.allowed_ports_map_fd as u32,
            key: &dport as *const _ as u64,
            value: &val as *const _ as u64,
            flags: 0,
        };
        let res = bpf_syscall(BPF_MAP_UPDATE_ELEM, &attr as *const _ as *const u8, std::mem::size_of::<BpfMapOpAttr>());
        if res < 0 {
            return Err(anyhow::anyhow!("allowed_ports map update failed: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

pub struct XdpRing {
    producer: *mut u32,
    consumer: *mut u32,
    descs: *mut libc::c_void,
    size: u32,
    mask: u32,
}

impl XdpRing {
    pub unsafe fn new(mmap_ptr: *mut libc::c_void, offsets: &XdpRingOffset, size: u32) -> Self {
        Self {
            producer: (mmap_ptr as usize + offsets.producer as usize) as *mut u32,
            consumer: (mmap_ptr as usize + offsets.consumer as usize) as *mut u32,
            descs: (mmap_ptr as usize + offsets.desc as usize) as *mut libc::c_void,
            size,
            mask: size - 1,
        }
    }

    #[inline(always)]
    pub fn producer_index(&self) -> u32 {
        // SAFETY: Atomic volatile read of UMEM ring producer pointer.
        unsafe { ptr::read_volatile(self.producer) }
    }

    #[inline(always)]
    pub fn consumer_index(&self) -> u32 {
        // SAFETY: Atomic volatile read of UMEM ring consumer pointer.
        unsafe { ptr::read_volatile(self.consumer) }
    }

    #[inline(always)]
    pub fn set_producer_index(&mut self, idx: u32) {
        // SAFETY: Atomic volatile write of UMEM ring producer pointer.
        unsafe { ptr::write_volatile(self.producer, idx) }
    }

    #[inline(always)]
    pub fn set_consumer_index(&mut self, idx: u32) {
        // SAFETY: Atomic volatile write of UMEM ring consumer pointer.
        unsafe { ptr::write_volatile(self.consumer, idx) }
    }
}

#[repr(C, align(64))]
pub struct SpscRecycleRing {
    buffer: [u64; TX_FRAMES],
    head: u32,
    tail: u32,
    mask: u32,
}

impl SpscRecycleRing {
    pub fn new() -> Self {
        let mut ring = Self {
            buffer: [0u64; TX_FRAMES],
            head: 0,
            tail: 0,
            mask: (TX_FRAMES - 1) as u32,
        };
        for i in 0..TX_FRAMES {
            ring.buffer[i] = ((RX_FRAMES + i) * FRAME_SIZE) as u64;
        }
        ring.tail = TX_FRAMES as u32;
        ring
    }

    #[inline(always)]
    pub fn pop(&mut self) -> Option<u64> {
        if self.head == self.tail {
            return None;
        }
        let addr = self.buffer[(self.head & self.mask) as usize];
        self.head = self.head.wrapping_add(1);
        Some(addr)
    }

    #[inline(always)]
    pub fn push(&mut self, addr: u64) -> bool {
        if self.tail.wrapping_sub(self.head) >= TX_FRAMES as u32 {
            return false;
        }
        self.buffer[(self.tail & self.mask) as usize] = addr;
        self.tail = self.tail.wrapping_add(1);
        true
    }
}

pub struct XdpSocket {
    fd: i32,
    umem_area: *mut u8,
    is_hugepage: bool,
    rx_ring: XdpRing,
    tx_ring: XdpRing,
    fill_ring: XdpRing,
    comp_ring: XdpRing,
    tx_recycle_ring: SpscRecycleRing,
    rx_cons: u32,
    tx_prod: u32,
    fill_prod: u32,
    comp_cons: u32,
}

impl XdpSocket {
    pub fn new(ifname: &str, queue_id: u32) -> anyhow::Result<Self> {
        // SAFETY: Raw initialization and memory mapping of AF_XDP socket.
        unsafe {
            let fd = libc::socket(libc::AF_XDP, libc::SOCK_RAW, 0);
            if fd < 0 {
                return Err(anyhow::anyhow!("Socket creation error"));
            }

            let mut is_hugepage = true;
            let mut umem_area = libc::mmap(
                ptr::null_mut(),
                UMEM_SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_HUGETLB | MAP_HUGE_2MB,
                -1,
                0,
            );

            if umem_area == libc::MAP_FAILED {
                is_hugepage = false;
                let mut aligned_ptr: *mut libc::c_void = ptr::null_mut();
                let ret = libc::posix_memalign(&mut aligned_ptr, 4096, UMEM_SIZE);
                if ret != 0 {
                    libc::close(fd);
                    return Err(anyhow::anyhow!("UMEM allocation failed"));
                }
                umem_area = aligned_ptr;
            }

            let _ = libc::mlock(umem_area, UMEM_SIZE);

            let reg = XdpUmemReg {
                addr: umem_area as u64,
                len: UMEM_SIZE as u64,
                chunk_size: FRAME_SIZE as u32,
                headroom: 0,
                flags: 0,
            };

            let res = libc::setsockopt(fd, SOL_XDP, XDP_UMEM_REG, &reg as *const _ as *const libc::c_void, std::mem::size_of::<XdpUmemReg>() as u32);
            if res < 0 {
                if is_hugepage {
                    libc::munmap(umem_area, UMEM_SIZE);
                } else {
                    libc::free(umem_area);
                }
                libc::close(fd);
                return Err(anyhow::anyhow!("setsockopt REG failed"));
            }

            let ring_size: u32 = 2048;
            libc::setsockopt(fd, SOL_XDP, XDP_UMEM_FILL_RING, &ring_size as *const _ as *const libc::c_void, 4);
            libc::setsockopt(fd, SOL_XDP, XDP_UMEM_COMPLETION_RING, &ring_size as *const _ as *const libc::c_void, 4);
            libc::setsockopt(fd, SOL_XDP, XDP_RX_RING, &ring_size as *const _ as *const libc::c_void, 4);
            libc::setsockopt(fd, SOL_XDP, XDP_TX_RING, &ring_size as *const _ as *const libc::c_void, 4);

            let mut offsets = XdpMmapOffsets::default();
            let mut optlen = std::mem::size_of::<XdpMmapOffsets>() as u32;
            libc::getsockopt(fd, SOL_XDP, XDP_MMAP_OFFSETS, &mut offsets as *mut _ as *mut libc::c_void, &mut optlen);

            let fill_map = libc::mmap(ptr::null_mut(), (offsets.fr.desc + (ring_size as u64 * 8)) as usize, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED | libc::MAP_POPULATE, fd, 0x100000000);
            let comp_map = libc::mmap(ptr::null_mut(), (offsets.cr.desc + (ring_size as u64 * 8)) as usize, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED | libc::MAP_POPULATE, fd, 0x180000000);
            let rx_map = libc::mmap(ptr::null_mut(), (offsets.rx.desc + (ring_size as u64 * std::mem::size_of::<XdpDesc>() as u64)) as usize, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED | libc::MAP_POPULATE, fd, 0x000000000);
            let tx_map = libc::mmap(ptr::null_mut(), (offsets.tx.desc + (ring_size as u64 * std::mem::size_of::<XdpDesc>() as u64)) as usize, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED | libc::MAP_POPULATE, fd, 0x080000000);

            let rx_ring = XdpRing::new(rx_map, &offsets.rx, ring_size);
            let tx_ring = XdpRing::new(tx_map, &offsets.tx, ring_size);
            let fill_ring = XdpRing::new(fill_map, &offsets.fr, ring_size);
            let comp_ring = XdpRing::new(comp_map, &offsets.cr, ring_size);

            let ifindex = {
                let cname = std::ffi::CString::new(ifname)?;
                libc::if_nametoindex(cname.as_ptr())
            };

            let sxdp = SockAddrXdp {
                sxdp_family: 44,
                sxdp_flags: XDP_COPY,
                sxdp_ifindex: ifindex,
                sxdp_queue_id: queue_id,
                sxdp_shared_umem_fd: 0,
            };

            libc::bind(fd, &sxdp as *const _ as *const libc::sockaddr, std::mem::size_of::<SockAddrXdp>() as u32);

            let mut socket = Self {
                fd,
                umem_area: umem_area as *mut u8,
                is_hugepage,
                rx_ring,
                tx_ring,
                fill_ring,
                comp_ring,
                tx_recycle_ring: SpscRecycleRing::new(),
                rx_cons: 0,
                tx_prod: 0,
                fill_prod: 0,
                comp_cons: 0,
            };

            socket.populate_fill_ring();
            Ok(socket)
        }
    }

    pub fn fd(&self) -> i32 {
        self.fd
    }

    pub fn is_hugepage(&self) -> bool {
        self.is_hugepage
    }

    fn populate_fill_ring(&mut self) {
        let prod = self.fill_prod;
        let ring_mask = self.fill_ring.mask;
        let raw_ring = self.fill_ring.descs as *mut u64;

        for i in 0..RX_FRAMES {
            let addr = (i * FRAME_SIZE) as u64;
            // SAFETY: Volatile write of recycled frame address back to fill ring.
            unsafe {
                ptr::write_volatile(raw_ring.add(((prod + i as u32) & ring_mask) as usize), addr);
            }
        }
        self.fill_prod = prod + RX_FRAMES as u32;
        fence(Ordering::Release);
        self.fill_ring.set_producer_index(self.fill_prod);
    }

    #[inline(always)]
    pub fn poll_read_zerocopy<F>(&mut self, max_batch: usize, mut on_packet: F) -> usize
    where
        F: FnMut(&[u8], &ParsedL4),
    {
        let rx_prod = self.rx_ring.producer_index();
        let available = rx_prod.wrapping_sub(self.rx_cons) as usize;
        if available == 0 {
            return 0;
        }

        fence(Ordering::Acquire);
        let batch_count = available.min(max_batch).min(BATCH_SIZE);
        let rx_descs = self.rx_ring.descs as *const XdpDesc;
        let fill_descs = self.fill_ring.descs as *mut u64;

        for i in 0..batch_count {
            let rx_idx = (self.rx_cons + i as u32) & self.rx_ring.mask;
            // SAFETY: Volatile descriptor load from mapped Rx ring memory.
            let desc = unsafe { ptr::read_volatile(rx_descs.add(rx_idx as usize)) };
            let len = desc.len as usize;

            // SAFETY: Deriving continuous raw slice representation directly from mapped UMEM packet.
            let packet_slice = unsafe {
                let ptr = self.umem_area.add(desc.addr as usize);
                std::slice::from_raw_parts(ptr, len)
            };

            let parsed = wire_simd::parse_one(packet_slice);
            on_packet(packet_slice, &parsed);

            let fill_idx = (self.fill_prod + i as u32) & self.fill_ring.mask;
            // SAFETY: Bypassing heap allocations and returning the completed frame pointer to the fill ring.
            unsafe {
                ptr::write_volatile(fill_descs.add(fill_idx as usize), desc.addr & !(FRAME_SIZE as u64 - 1));
            }
        }

        self.rx_cons += batch_count as u32;
        self.fill_prod += batch_count as u32;

        fence(Ordering::Release);
        self.rx_ring.set_consumer_index(self.rx_cons);
        self.fill_ring.set_producer_index(self.fill_prod);

        batch_count
    }

    #[inline(always)]
    pub fn poll_read_batch(&mut self, out_packets: &mut [Vec<u8>]) -> usize {
        let rx_prod = self.rx_ring.producer_index();
        let available = rx_prod.wrapping_sub(self.rx_cons) as usize;
        if available == 0 {
            return 0;
        }

        fence(Ordering::Acquire);
        let batch_count = available.min(out_packets.len()).min(BATCH_SIZE);
        let rx_descs = self.rx_ring.descs as *const XdpDesc;
        let fill_descs = self.fill_ring.descs as *mut u64;

        for i in 0..batch_count {
            let rx_idx = (self.rx_cons + i as u32) & self.rx_ring.mask;
            // SAFETY: Volatile read from raw descriptor queue array.
            let desc = unsafe { ptr::read_volatile(rx_descs.add(rx_idx as usize)) };
            let len = desc.len as usize;

            // SAFETY: Pointer math within valid UMEM address bounds.
            let umem_packet_ptr = unsafe { self.umem_area.add(desc.addr as usize) };
            let target_vec = &mut out_packets[i];
            target_vec.resize(len, 0);
            // SAFETY: Copying packet payload into allocated vector segment.
            unsafe {
                ptr::copy_nonoverlapping(umem_packet_ptr, target_vec.as_mut_ptr(), len);
            }

            let fill_idx = (self.fill_prod + i as u32) & self.fill_ring.mask;
            // SAFETY: Transferring ownership back to fill ring slot.
            unsafe {
                ptr::write_volatile(fill_descs.add(fill_idx as usize), desc.addr & !(FRAME_SIZE as u64 - 1));
            }
        }

        self.rx_cons += batch_count as u32;
        self.fill_prod += batch_count as u32;

        fence(Ordering::Release);
        self.rx_ring.set_consumer_index(self.rx_cons);
        self.fill_ring.set_producer_index(self.fill_prod);

        batch_count
    }

    pub fn write_async(&mut self, buf: &[u8]) -> std::io::Result<Option<usize>> {
        self.reclaim_completions();
        let t_tx = probes::stage_begin(StageId::TxEnqueue);

        let tx_cons = self.tx_ring.consumer_index();
        if self.tx_prod - tx_cons >= self.tx_ring.size {
            probes::stage_end(StageId::TxEnqueue, t_tx);
            return Ok(None);
        }

        let tx_addr = match self.tx_recycle_ring.pop() {
            Some(addr) => addr,
            None => {
                probes::stage_end(StageId::TxEnqueue, t_tx);
                return Ok(None);
            }
        };

        // SAFETY: Pointer math inside mapped memory area.
        let umem_packet_ptr = unsafe { self.umem_area.add(tx_addr as usize) };
        // SAFETY: Direct non-overlapping copy of outbound packet to UMEM segment.
        unsafe {
            ptr::copy_nonoverlapping(buf.as_ptr(), umem_packet_ptr, buf.len());
        }

        let frame_index = (self.tx_prod & self.tx_ring.mask) as usize;
        let tx_descs = self.tx_ring.descs as *mut XdpDesc;
        let desc = XdpDesc {
            addr: tx_addr,
            len: buf.len() as u32,
            options: 0,
        };

        // SAFETY: Volatile queue of Tx descriptor to the ring descriptor.
        unsafe {
            ptr::write_volatile(tx_descs.add(frame_index), desc);
        }

        self.tx_prod += 1;
        fence(Ordering::Release);
        self.tx_ring.set_producer_index(self.tx_prod);

        // SAFETY: Socket flush notification kick.
        unsafe {
            libc::send(self.fd, ptr::null(), 0, libc::MSG_DONTWAIT);
        }

        probes::stage_end(StageId::TxEnqueue, t_tx);
        Ok(Some(buf.len()))
    }

    fn reclaim_completions(&mut self) {
        let t_reap = probes::stage_begin(StageId::TxCompleteReap);
        let comp_prod = self.comp_ring.producer_index();
        let available = comp_prod.wrapping_sub(self.comp_cons);
        if available == 0 {
            probes::stage_end(StageId::TxCompleteReap, t_reap);
            return;
        }

        fence(Ordering::Acquire);
        let comp_descs = self.comp_ring.descs as *const u64;

        for i in 0..available {
            let idx = (self.comp_cons + i) & self.comp_ring.mask;
            // SAFETY: Volatile read of completed frame descriptor from completion ring.
            let completed_addr = unsafe { ptr::read_volatile(comp_descs.add(idx as usize)) };
            self.tx_recycle_ring.push(completed_addr);
        }

        self.comp_cons += available;
        fence(Ordering::Release);
        self.comp_ring.set_consumer_index(self.comp_cons);
        probes::stage_end(StageId::TxCompleteReap, t_reap);
    }
}

impl Drop for XdpSocket {
    fn drop(&mut self) {
        // SAFETY: Memory unmapping and file descriptor close operations.
        unsafe {
            libc::close(self.fd);
            if self.is_hugepage {
                libc::munmap(self.umem_area as *mut libc::c_void, UMEM_SIZE);
            } else {
                libc::free(self.umem_area as *mut libc::c_void);
            }
        }
    }
}
