use std::cell::RefCell;
use std::collections::HashMap;

/// Mock for x86 port I/O. Stores written bytes per port, returns pre-configured reads.
pub struct MockPort {
    /// For each port: queue of bytes to return on read
    read_queue: RefCell<HashMap<u16, Vec<u8>>>,
    /// For each port: all bytes written
    write_log: RefCell<HashMap<u16, Vec<u8>>>,
}

impl Default for MockPort {
    fn default() -> Self {
        Self::new()
    }
}

impl MockPort {
    pub fn new() -> Self {
        MockPort {
            read_queue: RefCell::new(HashMap::new()),
            write_log: RefCell::new(HashMap::new()),
        }
    }

    pub fn queue_read(&self, port: u16, bytes: Vec<u8>) {
        self.read_queue.borrow_mut().entry(port).or_default().extend(bytes);
    }

    pub fn read_u8(&self, port: u16) -> u8 {
        let mut q = self.read_queue.borrow_mut();
        q.get_mut(&port).and_then(|v| v.pop()).unwrap_or(0)
    }

    pub fn write_u8(&self, port: u16, value: u8) {
        self.write_log.borrow_mut().entry(port).or_default().push(value);
    }

    pub fn write_log(&self, port: u16) -> Vec<u8> {
        self.write_log.borrow().get(&port).cloned().unwrap_or_default()
    }

    pub fn clear(&self) {
        self.read_queue.borrow_mut().clear();
        self.write_log.borrow_mut().clear();
    }
}

/// Mock for memory-mapped I/O regions.
pub struct MockMmio {
    data: RefCell<Vec<u8>>,
    base: u64,
    size: usize,
}

impl MockMmio {
    pub fn new(base: u64, size: usize) -> Self {
        MockMmio { data: RefCell::new(vec![0u8; size]), base, size }
    }

    pub fn write_u8(&self, addr: u64, value: u8) {
        let offset = (addr - self.base) as usize;
        if offset < self.size {
            self.data.borrow_mut()[offset] = value;
        }
    }

    pub fn read_u8(&self, addr: u64) -> u8 {
        let offset = (addr - self.base) as usize;
        if offset < self.size { self.data.borrow()[offset] } else { 0 }
    }

    pub fn write_u32(&self, addr: u64, value: u32) {
        let offset = (addr - self.base) as usize;
        if offset + 4 <= self.size {
            let bytes = value.to_le_bytes();
            self.data.borrow_mut()[offset..offset + 4].copy_from_slice(&bytes);
        }
    }

    pub fn read_u32(&self, addr: u64) -> u32 {
        let offset = (addr - self.base) as usize;
        if offset + 4 <= self.size {
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&self.data.borrow()[offset..offset + 4]);
            u32::from_le_bytes(bytes)
        } else { 0 }
    }
}

/// Mock physical memory at a fixed offset (simulates kernel's PHYSICAL_MEMORY_OFFSET).
pub struct MockPhysMem {
    backing: RefCell<Vec<u8>>,
    offset: u64,
}

impl MockPhysMem {
    pub fn new(offset: u64, size: usize) -> Self {
        MockPhysMem { backing: RefCell::new(vec![0u8; size]), offset }
    }

    pub fn virt_to_phys(&self, virt: u64) -> Option<u64> {
        if virt >= self.offset {
            let phys = virt - self.offset;
            if (phys as usize) < self.backing.borrow().len() {
                return Some(phys);
            }
        }
        None
    }

    pub fn write_u8(&self, virt: u64, value: u8) {
        if let Some(phys) = self.virt_to_phys(virt) {
            self.backing.borrow_mut()[phys as usize] = value;
        }
    }

    pub fn read_u8(&self, virt: u64) -> u8 {
        self.virt_to_phys(virt).map(|p| self.backing.borrow()[p as usize]).unwrap_or(0)
    }

    pub fn write_u32(&self, virt: u64, value: u32) {
        if let Some(phys) = self.virt_to_phys(virt) {
            let bytes = value.to_le_bytes();
            let p = phys as usize;
            let len = self.backing.borrow().len();
            if p + 4 <= len {
                self.backing.borrow_mut()[p..p + 4].copy_from_slice(&bytes);
            }
        }
    }

    pub fn read_u32(&self, virt: u64) -> u32 {
        self.virt_to_phys(virt).map(|p| {
            let p = p as usize;
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&self.backing.borrow()[p..p + 4]);
            u32::from_le_bytes(bytes)
        }).unwrap_or(0)
    }
}

/// Convenience struct bundling all mock resources for a test.
pub struct MockHarness {
    pub ports: MockPort,
    pub phys_mem: MockPhysMem,
}

impl Default for MockHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl MockHarness {
    pub fn new() -> Self {
        MockHarness {
            ports: MockPort::new(),
            phys_mem: MockPhysMem::new(0xFFFF_8000_0000_0000, 64 * 1024 * 1024),
        }
    }

    pub fn clear(&self) {
        self.ports.clear();
    }
}
