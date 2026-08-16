use crate::Test;

// ---------------------------------------------------------------------------
// Port 1: ext2 block/inode allocation (`kernel/src/vfs/ext2.rs`).
//
// `allocate_block` walks the block groups; a group whose
// `bg_free_blocks_count` is zero is skipped outright. Within a group it
// scans the block-bitmap bytes, skips full bytes (0xFF), and takes the
// FIRST clear bit (LSB-first: `bit in 0..8`, `1 << bit`), setting it and
// decrementing both the group-descriptor free count and the superblock
// total. The returned block number is `g * blocks_per_group + byte*8 + bit`
// (0-based). `allocate_inode` is identical on the inode bitmap but returns
// `g * inodes_per_group + byte*8 + bit + 1` — inode numbers are 1-based
// (inode 0 is reserved). `free_block`/`free_inode` clear the bit via
// `block_group`/`inode_group` and bump both free counts. `inode_group` is
// `((inum - 1) / ipg, (inum - 1) % ipg)`; `block_group` is
// `(block / bpg, block % bpg)`.
// ---------------------------------------------------------------------------

struct GroupState {
    block_bitmap: Vec<u8>,
    inode_bitmap: Vec<u8>,
    free_blocks: u32,
    free_inodes: u32,
}

struct Ext2 {
    blocks_per_group: u32,
    inodes_per_group: u32,
    groups: Vec<GroupState>,
    /// Superblock `s_free_blocks_count`.
    free_blocks_total: u32,
    /// Superblock `s_free_inodes_count`.
    free_inodes_total: u32,
}

impl Ext2 {
    fn new(blocks_per_group: u32, inodes_per_group: u32, groups: u32) -> Self {
        Ext2 {
            blocks_per_group,
            inodes_per_group,
            groups: (0..groups)
                .map(|_| GroupState {
                    block_bitmap: vec![0u8; (blocks_per_group / 8) as usize],
                    inode_bitmap: vec![0u8; (inodes_per_group / 8) as usize],
                    free_blocks: blocks_per_group,
                    free_inodes: inodes_per_group,
                })
                .collect(),
            free_blocks_total: blocks_per_group * groups,
            free_inodes_total: inodes_per_group * groups,
        }
    }

    /// Mirrors `Ext2FileSystem::inode_group`.
    fn inode_group(&self, inum: u32) -> (u32, u32) {
        ((inum - 1) / self.inodes_per_group, (inum - 1) % self.inodes_per_group)
    }

    /// Mirrors `Ext2FileSystem::block_group`.
    fn block_group(&self, block: u32) -> (u32, u32) {
        (block / self.blocks_per_group, block % self.blocks_per_group)
    }

    /// Mirrors `Ext2FileSystem::allocate_block` (without the device I/O).
    fn allocate_block(&mut self) -> Result<u32, ()> {
        for g in 0..self.groups.len() {
            if self.groups[g].free_blocks == 0 {
                continue;
            }
            let bitmap_len = self.groups[g].block_bitmap.len();
            for byte_idx in 0..bitmap_len {
                if self.groups[g].block_bitmap[byte_idx] == 0xFF {
                    continue;
                }
                for bit in 0..8u8 {
                    if (self.groups[g].block_bitmap[byte_idx] & (1 << bit)) == 0 {
                        self.groups[g].block_bitmap[byte_idx] |= 1 << bit;
                        self.groups[g].free_blocks -= 1;
                        self.free_blocks_total -= 1;
                        return Ok(g as u32 * self.blocks_per_group
                            + (byte_idx as u32 * 8 + bit as u32));
                    }
                }
            }
        }
        Err(())
    }

    /// Mirrors `Ext2FileSystem::free_block`.
    fn free_block(&mut self, block: u32) {
        let (group, idx) = self.block_group(block);
        let byte = (idx / 8) as usize;
        let b = (idx % 8) as u8;
        self.groups[group as usize].block_bitmap[byte] &= !(1 << b);
        self.groups[group as usize].free_blocks += 1;
        self.free_blocks_total += 1;
    }

    /// Mirrors `Ext2FileSystem::allocate_inode` (1-based inode numbers).
    fn allocate_inode(&mut self) -> Result<u32, ()> {
        for g in 0..self.groups.len() {
            if self.groups[g].free_inodes == 0 {
                continue;
            }
            let bitmap_len = self.groups[g].inode_bitmap.len();
            for byte_idx in 0..bitmap_len {
                if self.groups[g].inode_bitmap[byte_idx] == 0xFF {
                    continue;
                }
                for bit in 0..8u8 {
                    if (self.groups[g].inode_bitmap[byte_idx] & (1 << bit)) == 0 {
                        self.groups[g].inode_bitmap[byte_idx] |= 1 << bit;
                        self.groups[g].free_inodes -= 1;
                        self.free_inodes_total -= 1;
                        return Ok(g as u32 * self.inodes_per_group
                            + (byte_idx as u32 * 8 + bit as u32 + 1));
                    }
                }
            }
        }
        Err(())
    }

    /// Mirrors `Ext2FileSystem::free_inode`.
    fn free_inode(&mut self, inum: u32) {
        let (group, idx) = self.inode_group(inum);
        let byte = (idx / 8) as usize;
        let b = (idx % 8) as u8;
        self.groups[group as usize].inode_bitmap[byte] &= !(1 << b);
        self.groups[group as usize].free_inodes += 1;
        self.free_inodes_total += 1;
    }
}

// ---------------------------------------------------------------------------
// Port 2: tarfs parsing + inode-tree lookup (`kernel/src/vfs/tarfs.rs`).
//
// `parse_tar` walks 512-byte headers: name is the NUL-terminated bytes
// 0..100, size is the octal field at 124..136 (`from_str_radix(_, 8)`,
// NUL/space terminated), type flag is byte 156 (`'5'` dir, `'2'` symlink,
// and a trailing '/' on the name also marks a dir), symlink target is the
// NUL-terminated bytes 157..317, and data follows the header padded to 512
// bytes. A zero first byte terminates the archive. `add_to_tree` splits the
// path on '/', filters empty and "." components, and walks/creates child
// nodes, finding existing children by name — a duplicate entry DESCENDS into
// the existing node without replacing its data (first entry wins).
// `TarNode::mode_bits`/`stat_size` mirror the VfsNode stat impl
// (S_IFLNK|0o777 / S_IFDIR|0o555 / S_IFREG|0o555; dirs default to 4096).
// ---------------------------------------------------------------------------

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

#[derive(Clone, Debug, PartialEq)]
struct TarNode {
    name: String,
    is_dir: bool,
    is_symlink: bool,
    link_target: Option<String>,
    data: Option<Vec<u8>>,
    children: Vec<TarNode>,
}

impl TarNode {
    fn new_dir(name: &str) -> Self {
        TarNode {
            name: name.to_string(),
            is_dir: true,
            is_symlink: false,
            link_target: None,
            data: None,
            children: Vec::new(),
        }
    }

    /// Mirrors `children.iter().find(|c| c.name == *comp)` — the linear
    /// inode lookup; first match wins.
    fn find(&self, name: &str) -> Option<&TarNode> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Mirrors the `TarNode::stat` st_mode computation.
    fn mode_bits(&self) -> u32 {
        if self.is_symlink {
            S_IFLNK | 0o777
        } else if self.is_dir {
            S_IFDIR | 0o555
        } else {
            S_IFREG | 0o555
        }
    }

    /// Mirrors the `TarNode::stat` st_size computation (dirs default 4096).
    fn stat_size(&self) -> i64 {
        if self.is_symlink {
            self.link_target.as_ref().map(|s| s.len() as i64).unwrap_or(0)
        } else {
            self.data.as_ref().map(|d| d.len() as i64).unwrap_or(4096)
        }
    }

    /// Mirrors the `TarNode::read` VfsNode impl.
    fn read(&self) -> Result<Vec<u8>, ()> {
        if self.is_symlink {
            return self.link_target.clone().map(|s| s.into_bytes()).ok_or(());
        }
        self.data.as_ref().cloned().ok_or(())
    }
}

/// Mirrors `parse_tar` (the serial log line is a kernel-only side effect).
fn parse_tar(data: &[u8]) -> TarNode {
    let mut root = TarNode::new_dir("/");
    let mut offset = 0;
    while offset + 512 <= data.len() {
        let header = &data[offset..offset + 512];
        if header[0] == 0 {
            break;
        }
        let name = {
            let end = header[..100].iter().position(|&b| b == 0).unwrap_or(100);
            core::str::from_utf8(&header[..end])
                .unwrap_or("")
                .trim_matches('\0')
                .to_string()
        };
        let size_str = {
            let end = header[124..136].iter().position(|&b| b == 0).unwrap_or(12);
            core::str::from_utf8(&header[124..124 + end])
                .unwrap_or("")
                .trim()
                .trim_matches('\0')
        };
        let size = usize::from_str_radix(size_str, 8).unwrap_or(0);
        let type_flag = header[156];
        let is_dir = type_flag == b'5' || name.ends_with('/');
        let is_symlink = type_flag == b'2';
        let link_target = if is_symlink {
            let end = header[157..317].iter().position(|&b| b == 0).unwrap_or(160);
            Some(
                core::str::from_utf8(&header[157..157 + end])
                    .unwrap_or("")
                    .trim_matches('\0')
                    .to_string(),
            )
        } else {
            None
        };
        let end = (offset + 512 + size).min(data.len());
        let file_data = if !is_dir && size > 0 { &data[offset + 512..end] } else { &[] };
        add_to_tree(&mut root, &name, is_dir, is_symlink, link_target, file_data);
        offset += 512 + ((size + 511) & !511);
    }
    root
}

/// Mirrors `add_to_tree`: split, filter, walk-or-create, first-wins.
fn add_to_tree(
    root: &mut TarNode,
    path: &str,
    is_dir: bool,
    is_symlink: bool,
    link_target: Option<String>,
    data: &[u8],
) {
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    if components.is_empty() {
        return;
    }
    let mut current = root;
    for (i, comp) in components.iter().enumerate() {
        let is_last = i == components.len() - 1;
        let existing = current.children.iter().position(|c| c.name == *comp);
        if let Some(idx) = existing {
            // Kernel behavior: descend into the existing node. Its data is
            // NOT replaced — the first entry for a name wins.
            current = &mut current.children[idx];
        } else {
            current.children.push(TarNode {
                name: comp.to_string(),
                is_dir: if is_last { is_dir } else { true },
                is_symlink: is_last && is_symlink,
                link_target: if is_last && is_symlink { link_target.clone() } else { None },
                data: if is_last && !is_dir && !is_symlink {
                    Some(data.to_vec())
                } else {
                    None
                },
                children: Vec::new(),
            });
            let idx = current.children.len() - 1;
            current = &mut current.children[idx];
        }
    }
}

/// Test double: builds a 512-byte tar header (name 0..100, octal size at
/// 124..136, type flag at 156, link target at 157..317) the way a tar
/// writer would, so `parse_tar` is exercised against spec-shaped input.
fn tar_header(name: &str, type_flag: u8, size: usize, link: Option<&str>) -> Vec<u8> {
    let mut h = [0u8; 512];
    let nb = name.as_bytes();
    h[..nb.len().min(100)].copy_from_slice(&nb[..nb.len().min(100)]);
    let octal = format!("{:011o}", size);
    let ob = octal.as_bytes();
    h[124..124 + ob.len()].copy_from_slice(ob);
    h[156] = type_flag;
    if let Some(t) = link {
        let tb = t.as_bytes();
        h[157..157 + tb.len().min(160)].copy_from_slice(&tb[..tb.len().min(160)]);
    }
    h.to_vec()
}

fn tar_archive(entries: &[(&str, u8, &[u8], Option<&str>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, flag, data, link) in entries {
        out.append(&mut tar_header(name, *flag, data.len(), *link));
        out.extend_from_slice(data);
        let pad = (512 - (data.len() % 512)) % 512;
        out.extend(std::iter::repeat_n(0u8, pad));
    }
    out.extend([0u8; 512]); // zero-header terminator
    out
}

// ---------------------------------------------------------------------------
// Port 3: ext2 indirect block traversal (`kernel/src/vfs/ext2.rs`).
//
// `read_all_block_indices` flattens an inode's block list: 12 direct
// pointers, then the single-indirect block (i_block[12]), double (13), and
// triple (14). `read_indirect(block, level)` reads the block, treats it as
// `entries = block_size/4` little-endian u32 pointers, and expands level 1
// to `entries` values; deeper levels recurse, expanding a ZERO pointer to
// `entries.pow(level - 1)` holes. `set_block_ptr` is the write side: it
// lazily allocates `start_block` when 0, walks down the chain, and writes
// the leaf pointer. `free_indirect` frees an index-block chain (never the
// data blocks -- those are freed separately via the flattened indices).
// `write_file_blocks` is the full write path ported below: it drives
// `set_block_ptr` from a byte buffer (direct + single/double/triple
// allocation, block reuse, shrink/truncate, i_size/i_blocks accounting) and
// the tests round-trip the data back through `read_all_block_indices`.
//
// One kernel quirk is pinned FAITHFULLY -- and one former quirk is pinned as
// FIXED -- so kernel behavior has a behavioral oracle:
//
// (a) CAPPED TRIPLE HOLE: a MISSING triple-indirect block (i_block[14] == 0)
//     contributes only `entries*entries` zeros -- NOT entries^3 -- per the
//     in-kernel ponytail comment ("an empty triple-indirect level is a pure
//     hole that no reader ever reaches ... Cap at level-2 size"). A PRESENT
//     triple still expands to the full entries^3.
//
// (b) FIXED -- level>=2 WRITES NO LONGER TRANSPOSE. `set_block_ptr` now
//     decomposes `idx` top-major: at a level-L block, `sub_idx = idx /
//     epb.pow(level-1)` and the recursion carries `idx % epb.pow(level-1)`.
//     Level-2 writes therefore land at flat `idx` (old code: `(idx%epb)*epb
//     + (idx/epb)`), and level-3 writes land at flat `idx` (old:
//     `(idx%epb)*epb^2 + ((idx/epb)%epb)*epb + idx/epb^2`). Level-1 writes
//     were never transposed. The tests below assert the round-trip identity:
//     a level-2/3 write for logical `idx` reads back at flat `idx`.
// ---------------------------------------------------------------------------

/// Host mirror of the ext2 `Inode`'s 15-block pointer array (12 direct +
/// single/double/triple indirect heads).
#[derive(Clone, Debug)]
struct IndirectInode {
    i_block: [u32; 15],
    i_size_lo: u32,
    i_blocks_lo: u32,
}

impl IndirectInode {
    fn new() -> Self {
        IndirectInode { i_block: [0; 15], i_size_lo: 0, i_blocks_lo: 0 }
    }
}

/// In-memory ext2 model for the indirect traversal: a block store plus a
/// monotonic allocator (the bitmap allocation itself is Port 1's concern, so
/// the block numbers here are arbitrary but unique).
struct IndirectFs {
    block_size: usize,
    blocks: std::collections::HashMap<u32, Vec<u8>>,
    next_block: u32,
}

impl IndirectFs {
    fn new(block_size: usize) -> Self {
        IndirectFs {
            block_size,
            blocks: std::collections::HashMap::new(),
            next_block: 1,
        }
    }

    /// Pointer entries per block: `block_size / 4`.
    fn entries(&self) -> usize {
        self.block_size / 4
    }

    /// Mirrors `Ext2FileSystem::read_block`; an absent block reads as zeros
    /// (a hole), matching the kernel's zero-filled index blocks.
    fn read_block(&self, block: u32) -> Vec<u8> {
        self.blocks
            .get(&block)
            .cloned()
            .unwrap_or_else(|| vec![0u8; self.block_size])
    }

    /// Mirrors `Ext2FileSystem::write_block`.
    fn write_block(&mut self, block: u32, data: &[u8]) {
        self.blocks.insert(block, data.to_vec());
    }

    /// Writes an index block from a pointer list (test seeding helper).
    fn seed_ptrs(&mut self, block: u32, ptrs: &[u32]) {
        let mut buf = vec![0u8; self.block_size];
        for (i, v) in ptrs.iter().take(self.entries()).enumerate() {
            buf[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.blocks.insert(block, buf);
    }

    /// Mirrors `Ext2FileSystem::allocate_block` (numbers are arbitrary here).
    fn allocate_block(&mut self) -> Result<u32, ()> {
        let b = self.next_block;
        self.next_block += 1;
        Ok(b)
    }

    /// Mirrors `Ext2FileSystem::free_block`: releases a data block. The
    /// model has no bitmap, so this just drops the stored bytes.
    fn free_block(&mut self, block_num: u32) -> Result<(), ()> {
        self.blocks.remove(&block_num);
        Ok(())
    }

    /// Mirrors `Ext2FileSystem::read_indirect` exactly, including the
    /// zero-pointer hole expansion `entries.pow(level - 1)`.
    fn read_indirect(&self, block_num: u32, level: u32) -> Result<Vec<u32>, ()> {
        let entries = self.entries();
        let buf = self.read_block(block_num);
        let mut out = Vec::new();
        if level == 1 {
            for i in 0..entries {
                out.push(u32::from_le_bytes(buf[i * 4..i * 4 + 4].try_into().unwrap()));
            }
        } else {
            for i in 0..entries {
                let p = u32::from_le_bytes(buf[i * 4..i * 4 + 4].try_into().unwrap());
                if p == 0 {
                    let sub_entries = entries.pow(level - 1);
                    out.extend(std::iter::repeat_n(0u32, sub_entries));
                } else {
                    out.append(&mut self.read_indirect(p, level - 1)?);
                }
            }
        }
        Ok(out)
    }

    /// Mirrors `Ext2FileSystem::set_block_ptr` EXACTLY -- top-major
    /// decomposition (`sub_idx = idx / epb.pow(level-1)`, recursion on
    /// `idx % epb.pow(level-1)`), matching `read_indirect`'s flattening so
    /// level>=2 writes land at their flat `idx` (quirk (b) above, FIXED).
    fn set_block_ptr(
        &mut self,
        start_block: &mut u32,
        level: u32,
        idx: usize,
        epb: usize,
        target: u32,
    ) -> Result<(), ()> {
        if *start_block == 0 {
            *start_block = self.allocate_block()?;
        }
        if level == 1 {
            let mut buf = self.read_block(*start_block);
            buf[idx * 4..idx * 4 + 4].copy_from_slice(&target.to_le_bytes());
            self.write_block(*start_block, &buf);
            Ok(())
        } else {
            let buf = self.read_block(*start_block);
            let span = epb.pow(level - 1);
            let sub_idx = idx / span;
            let mut sub = u32::from_le_bytes(buf[sub_idx * 4..sub_idx * 4 + 4].try_into().unwrap());
            Self::set_block_ptr(self, &mut sub, level - 1, idx % span, epb, target)?;
            let mut buf2 = self.read_block(*start_block);
            buf2[sub_idx * 4..sub_idx * 4 + 4].copy_from_slice(&sub.to_le_bytes());
            self.write_block(*start_block, &buf2);
            Ok(())
        }
    }

    /// Mirrors `Ext2FileSystem::read_all_block_indices` exactly, including
    /// the capped triple-indirect hole (i_block[14] == 0 -> entries^2 zeros,
    /// NOT entries^3 -- quirk (a) above).
    fn read_all_block_indices(&self, inode: &IndirectInode) -> Result<Vec<u32>, ()> {
        let mut blocks = Vec::new();
        for i in 0..12 {
            blocks.push(inode.i_block[i]);
        }
        let entries = self.entries();
        if inode.i_block[12] != 0 {
            blocks.append(&mut self.read_indirect(inode.i_block[12], 1)?);
        } else {
            blocks.extend(std::iter::repeat_n(0u32, entries));
        }
        if inode.i_block[13] != 0 {
            blocks.append(&mut self.read_indirect(inode.i_block[13], 2)?);
        } else {
            blocks.extend(std::iter::repeat_n(0u32, entries * entries));
        }
        if inode.i_block[14] != 0 {
            blocks.append(&mut self.read_indirect(inode.i_block[14], 3)?);
        } else {
            // kernel ponytail: an empty triple-indirect level is capped at
            // level-2 size (entries^2), not entries^3.
            blocks.extend(std::iter::repeat_n(0u32, entries * entries));
        }
        Ok(blocks)
    }

    /// Mirrors `Ext2FileSystem::free_indirect`: frees the index-block chain
    /// (recursing on nonzero pointers at level > 1), never the data blocks.
    fn free_indirect(&mut self, block_num: u32, level: u32) -> Result<(), ()> {
        let entries = self.entries();
        let buf = self.read_block(block_num);
        if level > 1 {
            for i in 0..entries {
                let p = u32::from_le_bytes(buf[i * 4..i * 4 + 4].try_into().unwrap());
                if p != 0 {
                    self.free_indirect(p, level - 1)?;
                }
            }
        }
        self.blocks.remove(&block_num);
        Ok(())
    }

    /// Mirrors `Ext2FileSystem::write_file_blocks` exactly: reuses existing
    /// data blocks (up to the flattened `read_all_block_indices` list),
    /// allocates direct (i < 12) or single/double/triple-indirect blocks
    /// (via `set_block_ptr`) for the rest, zero-pads each block to the
    /// block size, frees excess data blocks when the file shrinks (zeroing
    /// the now-unused DIRECT pointers; the kernel's own "ponytail" TODO
    /// leaves stale indirect chains behind), and updates `i_size_lo` /
    /// `i_blocks_lo`. This is the end-to-end write oracle for `set_block_ptr`.
    fn write_file_blocks(&mut self, inode: &mut IndirectInode, data: &[u8]) -> Result<(), ()> {
        let bs = self.block_size;
        let needed = if data.is_empty() { 0 } else { (data.len() + bs - 1) / bs };
        let epb = bs / 4;
        // Reuse existing blocks up to min(old_count, needed), then allocate new ones
        let old_blocks = self.read_all_block_indices(inode)?;
        for i in 0..needed {
            let off = i * bs;
            let len = std::cmp::min(bs, data.len() - off);
            let mut block_data = vec![0u8; bs];
            if len > 0 {
                block_data[..len].copy_from_slice(&data[off..off + len]);
            }
            let bnum = if i < old_blocks.len() && old_blocks[i] != 0 {
                old_blocks[i]
            } else if i < 12 {
                let nb = self.allocate_block()?;
                inode.i_block[i] = nb;
                nb
            } else {
                let ndb = self.allocate_block()?;
                let idx = i - 12;
                if idx < epb {
                    let mut blk = inode.i_block[12];
                    Self::set_block_ptr(self, &mut blk, 1, idx, epb, ndb)?;
                    inode.i_block[12] = blk;
                } else if idx < epb + epb * epb {
                    // The double-indirect region spans idx in [epb, epb+epb^2)
                    // (epb^2 entries), so its sub-flat is idx - epb.
                    let mut blk = inode.i_block[13];
                    Self::set_block_ptr(self, &mut blk, 2, idx - epb, epb, ndb)?;
                    inode.i_block[13] = blk;
                } else {
                    // Triple-indirect region base = epb + epb^2 (sub-flat is
                    // idx - epb - epb^2).
                    let mut blk = inode.i_block[14];
                    Self::set_block_ptr(self, &mut blk, 3, idx - epb - epb * epb, epb, ndb)?;
                    inode.i_block[14] = blk;
                }
                ndb
            };
            self.write_block(bnum, &block_data);
        }
        // Free excess blocks if data shrank
        if needed < old_blocks.len() {
            for &b in &old_blocks[needed..] {
                if b != 0 {
                    self.free_block(b)?;
                }
            }
            // Zero out now-unused inode block pointers
            for i in needed..12 {
                if i < old_blocks.len() {
                    inode.i_block[i] = 0;
                }
            }
            // ponytail: indirect block pointer cleanup for large files -- add
            // when needed (mirrors the kernel's own TODO).
        }
        inode.i_size_lo = data.len() as u32;
        inode.i_blocks_lo = (if needed == 0 { 0 } else { needed * bs / 512 }) as u32;
        Ok(())
    }

    /// Round-trip reader for the write tests: flattens the inode's block
    /// list with `read_all_block_indices` (the kernel's canonical read side)
    /// and concatenates each data block, treating a zero index as a hole of
    /// `block_size` zeros, then truncates to `i_size_lo`.
    fn read_file_data(&self, inode: &IndirectInode) -> Result<Vec<u8>, ()> {
        let mut out = Vec::new();
        for b in self.read_all_block_indices(inode)? {
            if b == 0 {
                out.extend(std::iter::repeat_n(0u8, self.block_size));
            } else {
                out.extend_from_slice(&self.read_block(b));
            }
        }
        out.truncate(inode.i_size_lo as usize);
        Ok(out)
    }
}

pub fn tests() -> Vec<Test> {
    vec![
        // ------------------------------------------------------------- ext2
        Test {
            name: "ext2_allocate_block_first_free_bit",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = Ext2::new(64, 64, 2);
                assert_eq_result!(fs.allocate_block(), Ok(0u32), "block 0, byte 0, bit 0");
                assert_eq_result!(fs.free_blocks_total, 127, "superblock count decremented");
                assert_eq_result!(fs.groups[0].free_blocks, 63, "group count decremented");
                Ok(())
            }),
        },
        Test {
            name: "ext2_allocate_block_skips_full_bytes",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = Ext2::new(64, 64, 2);
                for i in 0..8u32 {
                    assert_eq_result!(fs.allocate_block(), Ok(i), "blocks 0..=7");
                }
                assert_eq_result!(
                    fs.allocate_block(),
                    Ok(8u32),
                    "byte 0 full (0xFF) is skipped, byte 1 bit 0"
                );
                Ok(())
            }),
        },
        Test {
            name: "ext2_allocate_block_lsb_within_byte",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = Ext2::new(64, 64, 2);
                for i in 0..8u32 {
                    assert_eq_result!(fs.allocate_block(), Ok(i));
                }
                fs.free_block(3);
                assert_eq_result!(
                    fs.allocate_block(),
                    Ok(3u32),
                    "bit 3 is now the lowest free bit — LSB-first scan"
                );
                assert_eq_result!(fs.allocate_block(), Ok(8u32), "then byte 1 bit 0");
                Ok(())
            }),
        },
        Test {
            name: "ext2_allocate_block_spans_groups",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = Ext2::new(64, 64, 2);
                for i in 0..64u32 {
                    assert_eq_result!(fs.allocate_block(), Ok(i), "group 0 fills 0..=63");
                }
                assert_eq_result!(
                    fs.allocate_block(),
                    Ok(64u32),
                    "group 0 exhausted -> group 1, g * blocks_per_group"
                );
                assert_eq_result!(fs.free_blocks_total, 63);
                Ok(())
            }),
        },
        Test {
            name: "ext2_allocate_block_group_free_count_short_circuits",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = Ext2::new(64, 64, 2);
                // A group whose bg_free_blocks_count is 0 is skipped WITHOUT
                // scanning its bitmap — even if the bitmap has clear bits.
                fs.groups[0].free_blocks = 0;
                assert_eq_result!(fs.allocate_block(), Ok(64u32), "skips straight to group 1");
                assert_eq_result!(fs.groups[1].free_blocks, 63);
                Ok(())
            }),
        },
        Test {
            name: "ext2_allocate_block_exhaustion",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = Ext2::new(64, 64, 2);
                for _ in 0..128u32 {
                    assert_eq_result!(fs.allocate_block().is_ok(), true);
                }
                assert_eq_result!(fs.free_blocks_total, 0);
                assert_eq_result!(fs.allocate_block(), Err(()), "all groups full");
                Ok(())
            }),
        },
        Test {
            name: "ext2_free_block_reuse_lowest",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = Ext2::new(64, 64, 2);
                assert_eq_result!(fs.allocate_block(), Ok(0u32));
                fs.free_block(0);
                assert_eq_result!(fs.free_blocks_total, 128, "free count restored");
                assert_eq_result!(fs.groups[0].free_blocks, 64);
                assert_eq_result!(fs.allocate_block(), Ok(0u32), "freed block is reused first");
                Ok(())
            }),
        },
        Test {
            name: "ext2_group_mapping_math",
            category: "kernel::fs",
            run: Box::new(|| {
                let fs = Ext2::new(64, 64, 2);
                assert_eq_result!(fs.inode_group(1), (0, 0), "inode 1 is group 0, index 0");
                assert_eq_result!(fs.inode_group(64), (0, 63), "last inode of group 0");
                assert_eq_result!(fs.inode_group(65), (1, 0), "first inode of group 1");
                assert_eq_result!(fs.block_group(0), (0, 0));
                assert_eq_result!(fs.block_group(63), (0, 63));
                assert_eq_result!(fs.block_group(64), (1, 0), "blocks are 0-based");
                Ok(())
            }),
        },
        Test {
            name: "ext2_allocate_inode_one_based_numbering",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = Ext2::new(64, 64, 2);
                assert_eq_result!(fs.allocate_inode(), Ok(1u32), "inode 0 is reserved");
                assert_eq_result!(fs.allocate_inode(), Ok(2u32));
                for _ in 0..61u32 {
                    fs.allocate_inode().unwrap(); // inodes 3..=63
                }
                assert_eq_result!(fs.allocate_inode(), Ok(64u32), "last inode of group 0");
                assert_eq_result!(fs.allocate_inode(), Ok(65u32), "group 1 starts at 65");
                fs.free_inode(2);
                assert_eq_result!(fs.free_inodes_total, 64, "65 allocated, one freed");
                assert_eq_result!(
                    fs.allocate_inode(),
                    Ok(2u32),
                    "freed inode is reused (inode_group maps it back)"
                );
                Ok(())
            }),
        },
        Test {
            name: "ext2_allocate_inode_exhaustion",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = Ext2::new(64, 64, 2);
                for _ in 0..128u32 {
                    assert_eq_result!(fs.allocate_inode().is_ok(), true);
                }
                assert_eq_result!(fs.free_inodes_total, 0);
                assert_eq_result!(fs.allocate_inode(), Err(()));
                Ok(())
            }),
        },
        // ------------------------------------------------------------- tarfs
        Test {
            name: "tarfs_octal_size_field",
            category: "kernel::fs",
            run: Box::new(|| {
                // "00000000144" octal = 100 decimal.
                let mut h = [0u8; 512];
                h[124..136].copy_from_slice(b"00000000144\0");
                let end = h[124..136].iter().position(|&b| b == 0).unwrap_or(12);
                let s = core::str::from_utf8(&h[124..124 + end]).unwrap().trim();
                assert_eq_result!(usize::from_str_radix(s, 8), Ok(100usize));
                Ok(())
            }),
        },
        Test {
            name: "tarfs_parse_name_field",
            category: "kernel::fs",
            run: Box::new(|| {
                let root = parse_tar(&tar_archive(&[("hello.txt", b'0', b"hi", None)]));
                let f = root.find("hello.txt").expect("entry found");
                assert_eq_result!(&f.name, "hello.txt");
                assert_eq_result!(f.is_dir, false);
                assert_eq_result!(f.read(), Ok(b"hi".to_vec()));
                Ok(())
            }),
        },
        Test {
            name: "tarfs_type_flag_classification",
            category: "kernel::fs",
            run: Box::new(|| {
                let root = parse_tar(&tar_archive(&[
                    ("dir1", b'5', b"", None),
                    ("dir2/", b'0', b"", None), // trailing slash marks a dir
                    ("lnk", b'2', b"", Some("/target")),
                    ("reg", b'0', b"x", None),
                ]));
                assert_eq_result!(root.find("dir1").unwrap().is_dir, true, "type flag '5'");
                assert_eq_result!(root.find("dir2").unwrap().is_dir, true, "trailing '/'");
                let lnk = root.find("lnk").unwrap();
                assert_eq_result!(lnk.is_symlink, true);
                assert_eq_result!(lnk.link_target.as_deref(), Some("/target"));
                assert_eq_result!(root.find("reg").unwrap().is_dir, false);
                Ok(())
            }),
        },
        Test {
            name: "tarfs_zero_header_terminates",
            category: "kernel::fs",
            run: Box::new(|| {
                // One real entry with NO data (so the next header lands exactly
                // at offset 512), followed by a genuine zero block: parsing
                // must stop at the zero first byte -- not keep walking (a
                // size-2 entry would advance past the block, exiting only via
                // the length bound and never exercising the terminator).
                let mut data = tar_header("first", b'0', 0, None);
                data.extend_from_slice(&[0u8; 512]); // zero header block
                let root = parse_tar(&data);
                assert_eq_result!(root.find("first").is_some(), true);
                assert_eq_result!(root.children.len(), 1, "zero header stops the walk");
                Ok(())
            }),
        },
        Test {
            name: "tarfs_data_padding_and_next_entry",
            category: "kernel::fs",
            run: Box::new(|| {
                // 3 bytes of data pad to 512; the next entry starts at 1024.
                let root = parse_tar(&tar_archive(&[
                    ("a", b'0', b"abc", None),
                    ("b", b'0', b"xyz", None),
                ]));
                assert_eq_result!(root.find("a").unwrap().read(), Ok(b"abc".to_vec()));
                assert_eq_result!(root.find("b").unwrap().read(), Ok(b"xyz".to_vec()));
                assert_eq_result!(root.children.len(), 2);
                Ok(())
            }),
        },
        Test {
            name: "tarfs_nested_path_creates_dirs",
            category: "kernel::fs",
            run: Box::new(|| {
                let root = parse_tar(&tar_archive(&[("a/b/c.txt", b'0', b"deep", None)]));
                let a = root.find("a").expect("intermediate dir created");
                assert_eq_result!(a.is_dir, true, "intermediate components become dirs");
                let b = a.find("b").expect("second level");
                assert_eq_result!(b.is_dir, true);
                let c = b.find("c.txt").expect("leaf file");
                assert_eq_result!(c.is_dir, false);
                assert_eq_result!(c.read(), Ok(b"deep".to_vec()));
                Ok(())
            }),
        },
        Test {
            name: "tarfs_duplicate_entry_first_wins",
            category: "kernel::fs",
            run: Box::new(|| {
                // Two entries for the same name: add_to_tree descends into the
                // existing node and never replaces its data.
                let root = parse_tar(&tar_archive(&[
                    ("f", b'0', b"first", None),
                    ("f", b'0', b"second", None),
                ]));
                let f = root.find("f").unwrap();
                assert_eq_result!(f.read(), Ok(b"first".to_vec()), "first entry wins");
                assert_eq_result!(root.children.len(), 1, "no duplicate node");
                Ok(())
            }),
        },
        Test {
            name: "tarfs_symlink_target_and_stat",
            category: "kernel::fs",
            run: Box::new(|| {
                let root = parse_tar(&tar_archive(&[("sh", b'2', b"", Some("/usr/bin/sh"))]));
                let lnk = root.find("sh").unwrap();
                assert_eq_result!(lnk.read(), Ok(b"/usr/bin/sh".to_vec()), "read follows link");
                assert_eq_result!(lnk.mode_bits(), S_IFLNK | 0o777);
                assert_eq_result!(lnk.stat_size(), 11, "size is the target length");
                Ok(())
            }),
        },
        Test {
            name: "tarfs_stat_mode_and_size_defaults",
            category: "kernel::fs",
            run: Box::new(|| {
                let root = parse_tar(&tar_archive(&[
                    ("d", b'5', b"", None),
                    ("f", b'0', b"0123456789", None),
                ]));
                let d = root.find("d").unwrap();
                assert_eq_result!(d.mode_bits(), S_IFDIR | 0o555);
                assert_eq_result!(d.stat_size(), 4096, "dirs default to 4096");
                let f = root.find("f").unwrap();
                assert_eq_result!(f.mode_bits(), S_IFREG | 0o555);
                assert_eq_result!(f.stat_size(), 10, "file size is the data length");
                Ok(())
            }),
        },
        Test {
            name: "tarfs_path_component_filtering",
            category: "kernel::fs",
            run: Box::new(|| {
                // Empty, ".", and leading "/" components are filtered; "./x"
                // and "/x/y" both resolve to the same tree shape.
                let root = parse_tar(&tar_archive(&[
                    ("./x/./y", b'0', b"dot", None),
                    ("/etc/passwd", b'0', b"root", None),
                ]));
                let x = root.find("x").unwrap();
                assert_eq_result!(x.is_dir, true);
                assert_eq_result!(x.find("y").unwrap().read(), Ok(b"dot".to_vec()));
                let etc = root.find("etc").unwrap();
                assert_eq_result!(etc.find("passwd").unwrap().read(), Ok(b"root".to_vec()));
                assert_eq_result!(root.children.len(), 2);
                Ok(())
            }),
        },
        // ------------------------------------------- ext2 indirect blocks
        Test {
            name: "ext2_read_indirect_level1_flattens_pointers",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = IndirectFs::new(16); // entries = 4
                fs.seed_ptrs(5, &[7, 0, 9, 3]);
                assert_eq_result!(fs.read_indirect(5, 1), Ok(vec![7, 0, 9, 3]));
                Ok(())
            }),
        },
        Test {
            name: "ext2_read_indirect_level2_expands_subblocks",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = IndirectFs::new(16);
                fs.seed_ptrs(1, &[2, 0, 3, 0]);
                fs.seed_ptrs(2, &[10, 11, 12, 13]);
                fs.seed_ptrs(3, &[20, 21, 22, 23]);
                assert_eq_result!(
                    fs.read_indirect(1, 2),
                    Ok(vec![10, 11, 12, 13, 0, 0, 0, 0, 20, 21, 22, 23, 0, 0, 0, 0]),
                    "top-major flatten; a zero pointer expands to entries holes"
                );
                Ok(())
            }),
        },
        Test {
            name: "ext2_read_indirect_level2_hole_size_is_entries",
            category: "kernel::fs",
            run: Box::new(|| {
                // A level-2 zero pointer contributes entries.pow(2-1) = entries
                // holes; a top block of all zeros still yields entries^2 total.
                let mut fs = IndirectFs::new(16);
                fs.seed_ptrs(1, &[0, 0, 0, 0]);
                assert_eq_result!(
                    fs.read_indirect(1, 2),
                    Ok(vec![0u32; 16]),
                    "4 zero pointers x 4 holes each"
                );
                Ok(())
            }),
        },
        Test {
            name: "ext2_read_indirect_level3_fanout_entries_cubed",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = IndirectFs::new(16);
                // triple -> double -> single chain, rest holes.
                fs.seed_ptrs(1, &[2, 0, 0, 0]);
                fs.seed_ptrs(2, &[3, 0, 0, 0]);
                fs.seed_ptrs(3, &[5, 6, 7, 8]);
                let out = fs
                    .read_indirect(1, 3)
                    .map_err(|_| "read_indirect failed")?;
                assert_eq_result!(out.len(), 64, "entries^3 = 4^3");
                assert_eq_result!(&out[0..4], &[5, 6, 7, 8][..]);
                assert_eq_result!(&out[4..64], &[0u32; 60][..], "all holes beyond the chain");
                Ok(())
            }),
        },
        Test {
            name: "ext2_read_all_block_indices_direct_layout",
            category: "kernel::fs",
            run: Box::new(|| {
                let fs = IndirectFs::new(16);
                let mut inode = IndirectInode::new();
                for i in 0..12u32 {
                    inode.i_block[i as usize] = 100 + i;
                }
                let idx = fs
                    .read_all_block_indices(&inode)
                    .map_err(|_| "read_all failed")?;
                assert_eq_result!(
                    idx.len(),
                    48,
                    "12 direct + 4 single-hole + 16 double-hole + 16 capped triple-hole"
                );
                assert_eq_result!(
                    &idx[0..12],
                    &[100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111][..]
                );
                assert_eq_result!(&idx[12..48], &[0u32; 36][..]);
                Ok(())
            }),
        },
        Test {
            name: "ext2_read_all_block_indices_single_indirect",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = IndirectFs::new(16);
                fs.seed_ptrs(5, &[1, 2, 3, 4]);
                let mut inode = IndirectInode::new();
                inode.i_block[12] = 5;
                let idx = fs
                    .read_all_block_indices(&inode)
                    .map_err(|_| "read_all failed")?;
                assert_eq_result!(&idx[12..16], &[1, 2, 3, 4][..], "single-indirect fanout");
                assert_eq_result!(&idx[16..48], &[0u32; 32][..]);
                Ok(())
            }),
        },
        Test {
            name: "ext2_read_all_empty_triple_hole_capped_at_level2",
            category: "kernel::fs",
            run: Box::new(|| {
                // Quirk (a): an absent triple-indirect block contributes
                // entries^2 = 16 zeros, NOT entries^3 = 64.
                let mut fs = IndirectFs::new(16);
                let mut inode = IndirectInode::new();
                inode.i_block[12] = 5; // present single, so the only hole is triple
                fs.seed_ptrs(5, &[0, 0, 0, 0]);
                let idx = fs
                    .read_all_block_indices(&inode)
                    .map_err(|_| "read_all failed")?;
                assert_eq_result!(idx.len(), 48, "12 + 4 + 16 + capped 16");
                // Contrast: a PRESENT triple would flatten to 12 + 4 + 16 + 64 = 96.
                let mut fs2 = IndirectFs::new(16);
                fs2.seed_ptrs(1, &[0, 0, 0, 0]); // triple head, all holes
                let mut inode2 = IndirectInode::new();
                inode2.i_block[14] = 1;
                let idx2 = fs2
                    .read_all_block_indices(&inode2)
                    .map_err(|_| "read_all failed")?;
                assert_eq_result!(idx2.len(), 96, "present triple expands to entries^3");
                Ok(())
            }),
        },
        Test {
            name: "ext2_set_block_ptr_level1_writes_leaf",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = IndirectFs::new(16);
                let mut top = 0u32;
                assert_eq_result!(
                    fs.set_block_ptr(&mut top, 1, 2, 4, 99),
                    Ok(()),
                    "level-1 set cannot fail"
                );
                assert_eq_result!(top, 1, "start block allocated on demand");
                assert_eq_result!(fs.read_indirect(top, 1), Ok(vec![0, 0, 99, 0]));
                Ok(())
            }),
        },
        Test {
            name: "ext2_set_block_ptr_level1_preserves_siblings",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = IndirectFs::new(16);
                fs.seed_ptrs(7, &[1, 2, 3, 4]);
                let mut top = 7u32;
                assert_eq_result!(fs.set_block_ptr(&mut top, 1, 1, 4, 77), Ok(()));
                assert_eq_result!(top, 7, "existing start block is kept");
                assert_eq_result!(fs.read_indirect(top, 1), Ok(vec![1, 77, 3, 4]));
                Ok(())
            }),
        },
        Test {
            name: "ext2_set_block_ptr_level2_lands_at_flat_idx",
            category: "kernel::fs",
            run: Box::new(|| {
                // Corrected semantics (quirk (b) FIXED): idx=4 (epb=4)
                // decomposes top-major to top[4/4=1], sub[4%4=0], so the
                // target lands at flat 4 (the old transposed code wrote
                // top[4%4=0], sub[4/4=1] -> flat 1).
                let mut fs = IndirectFs::new(16);
                let mut top = 0u32;
                assert_eq_result!(fs.set_block_ptr(&mut top, 2, 4, 4, 555), Ok(()));
                let mut inode = IndirectInode::new();
                inode.i_block[13] = top;
                let idx = fs
                    .read_all_block_indices(&inode)
                    .map_err(|_| "read_all failed")?;
                // read_all position = 12 direct + 4 single-hole + flat index.
                assert_eq_result!(idx[12 + 4 + 4], 555, "level-2 idx 4 -> flat 4");
                assert_eq_result!(idx[17], 0, "flat 1 stays a hole (was the transposed spot)");
                assert_eq_result!(fs.blocks.len(), 2, "top + one sub block allocated");
                Ok(())
            }),
        },
        Test {
            name: "ext2_set_block_ptr_level2_idx_zero_first_slot",
            category: "kernel::fs",
            run: Box::new(|| {
                // idx=0 decomposes to top[0/4=0], sub[0%4=0]: the first
                // double-indirect slot (identity under any decomposition).
                let mut fs = IndirectFs::new(16);
                let mut top = 0u32;
                assert_eq_result!(fs.set_block_ptr(&mut top, 2, 0, 4, 888), Ok(()));
                let mut inode = IndirectInode::new();
                inode.i_block[13] = top;
                let idx = fs
                    .read_all_block_indices(&inode)
                    .map_err(|_| "read_all failed")?;
                assert_eq_result!(idx[12 + 4], 888, "first double-indirect slot, flat 0");
                Ok(())
            }),
        },
        Test {
            name: "ext2_set_block_ptr_level3_lands_at_flat_idx",
            category: "kernel::fs",
            run: Box::new(|| {
                // Corrected semantics at depth 3: top-major decomposition with
                // span = epb^2 at the triple level. idx=1 -> triple[1/16=0]
                // level2[(1%16)/4=0] leaf[1%4=1], i.e. flat 1; idx=20 ->
                // triple[20/16=1] level2[(20%16)/4=1] leaf[20%4=0], i.e.
                // flat 20. Both pin the fix: the old transposed code landed
                // them at flat 16 / flat 80.
                let mut fs = IndirectFs::new(16);
                let mut top = 0u32;
                assert_eq_result!(fs.set_block_ptr(&mut top, 3, 1, 4, 444), Ok(()));
                assert_eq_result!(fs.set_block_ptr(&mut top, 3, 20, 4, 445), Ok(()));
                let mut inode = IndirectInode::new();
                inode.i_block[14] = top;
                let idx = fs
                    .read_all_block_indices(&inode)
                    .map_err(|_| "read_all failed")?;
                assert_eq_result!(idx.len(), 96, "a PRESENT triple expands to entries^3");
                assert_eq_result!(idx[12 + 4 + 16 + 1], 444, "level-3 idx 1 -> flat 1");
                assert_eq_result!(idx[12 + 4 + 16 + 20], 445, "level-3 idx 20 -> flat 20");
                assert_eq_result!(fs.blocks.len(), 5, "triple + two level-2 + two level-1 blocks");
                Ok(())
            }),
        },
        Test {
            name: "ext2_set_block_ptr_level1_round_trip_read_all",
            category: "kernel::fs",
            run: Box::new(|| {
                // Level-1 writes are NOT transposed: a full single-indirect file
                // (12 direct + epb indirect) reads back exactly as written.
                let mut fs = IndirectFs::new(16);
                let mut top = 0u32;
                for i in 0..4 {
                    assert_eq_result!(fs.set_block_ptr(&mut top, 1, i, 4, 1000 + i as u32), Ok(()));
                }
                let mut inode = IndirectInode::new();
                inode.i_block[12] = top;
                let idx = fs
                    .read_all_block_indices(&inode)
                    .map_err(|_| "read_all failed")?;
                assert_eq_result!(&idx[12..16], &[1000, 1001, 1002, 1003][..]);
                Ok(())
            }),
        },
        Test {
            name: "ext2_free_indirect_level1_removes_block",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = IndirectFs::new(16);
                fs.seed_ptrs(5, &[1, 2, 3, 4]);
                assert_eq_result!(fs.free_indirect(5, 1), Ok(()));
                assert_eq_result!(fs.blocks.contains_key(&5), false, "level-1 index freed");
                Ok(())
            }),
        },
        Test {
            name: "ext2_free_indirect_level2_frees_chain",
            category: "kernel::fs",
            run: Box::new(|| {
                let mut fs = IndirectFs::new(16);
                let mut top = 0u32;
                assert_eq_result!(fs.set_block_ptr(&mut top, 2, 2, 4, 999), Ok(()));
                assert_eq_result!(fs.blocks.len(), 2, "top + sub before the free");
                assert_eq_result!(fs.free_indirect(top, 2), Ok(()));
                assert_eq_result!(fs.blocks.len(), 0, "top and its sub block both freed");
                Ok(())
            }),
        },
        Test {
            name: "ext2_write_file_blocks_direct_round_trip",
            category: "kernel::fs",
            run: Box::new(|| {
                // 40 bytes = 3 whole blocks (bs=16): all direct pointers, no
                // indirect heads, i_blocks_lo stays 0 (3*16/512 = 0).
                let mut fs = IndirectFs::new(16);
                let mut inode = IndirectInode::new();
                let data: Vec<u8> = (0..40u8).collect();
                assert_eq_result!(fs.write_file_blocks(&mut inode, &data), Ok(()));
                assert_eq_result!(inode.i_size_lo, 40);
                assert_eq_result!(inode.i_blocks_lo, 0, "3*16/512 = 0 in 512-unit accounting");
                assert_eq_result!(inode.i_block[0] != 0, true, "direct pointer 0 set");
                assert_eq_result!(inode.i_block[2] != 0, true, "direct pointer 2 set");
                assert_eq_result!(inode.i_block[3], 0, "block 3 not touched");
                assert_eq_result!(inode.i_block[12], 0, "single-indirect head untouched");
                assert_eq_result!(fs.read_file_data(&inode).map_err(|_| "read failed")?, data, "direct round-trip");
                Ok(())
            }),
        },
        Test {
            name: "ext2_write_file_blocks_single_indirect_round_trip",
            category: "kernel::fs",
            run: Box::new(|| {
                // 14 blocks: direct 0..11 plus single-indirect idx 0..1
                // (blocks 12..13) via i_block[12]. Round-trip must be exact.
                let mut fs = IndirectFs::new(16);
                let mut inode = IndirectInode::new();
                let data: Vec<u8> = (0..(14 * 16)).map(|i| (i % 251) as u8).collect();
                assert_eq_result!(fs.write_file_blocks(&mut inode, &data), Ok(()));
                assert_eq_result!(inode.i_block[12] != 0, true, "single-indirect head allocated");
                assert_eq_result!(inode.i_block[13], 0, "double-indirect head untouched");
                assert_eq_result!(fs.read_file_data(&inode).map_err(|_| "read failed")?, data, "single-indirect round-trip");
                Ok(())
            }),
        },
        Test {
            name: "ext2_write_file_blocks_double_indirect_round_trip",
            category: "kernel::fs",
            run: Box::new(|| {
                // 20 blocks crosses into the double-indirect region (idx 4..7
                // at level 2). A patterned round-trip catches the old
                // set_block_ptr transpose: a level-2 write for idx 4 landed
                // at flat 1, corrupting bytes 16..32 of the read-back.
                let mut fs = IndirectFs::new(16);
                let mut inode = IndirectInode::new();
                let data: Vec<u8> = (0..(20 * 16)).map(|i| (i % 251) as u8).collect();
                assert_eq_result!(fs.write_file_blocks(&mut inode, &data), Ok(()));
                assert_eq_result!(inode.i_block[13] != 0, true, "double-indirect head allocated");
                assert_eq_result!(fs.read_file_data(&inode).map_err(|_| "read failed")?, data, "double-indirect round-trip");
                Ok(())
            }),
        },
        Test {
            name: "ext2_write_file_blocks_triple_indirect_round_trip",
            category: "kernel::fs",
            run: Box::new(|| {
                // 40 blocks crosses into the triple-indirect region (idx 16..27
                // at level 3). The old code transposed these writes entirely;
                // the patterned round-trip must match exactly.
                let mut fs = IndirectFs::new(16);
                let mut inode = IndirectInode::new();
                let data: Vec<u8> = (0..(40 * 16)).map(|i| (i % 251) as u8).collect();
                assert_eq_result!(fs.write_file_blocks(&mut inode, &data), Ok(()));
                assert_eq_result!(inode.i_block[14] != 0, true, "triple-indirect head allocated");
                assert_eq_result!(inode.i_blocks_lo, 1, "40*16/512 = 1 in 512-unit accounting");
                assert_eq_result!(fs.read_file_data(&inode).map_err(|_| "read failed")?, data, "triple-indirect round-trip");
                Ok(())
            }),
        },
        Test {
            name: "ext2_write_file_blocks_reuses_existing_blocks",
            category: "kernel::fs",
            run: Box::new(|| {
                // Same-size rewrite reuses every existing data block: the
                // allocator must not hand out a single new block, and the
                // new pattern must read back.
                let mut fs = IndirectFs::new(16);
                let mut inode = IndirectInode::new();
                let first: Vec<u8> = (0..(20 * 16)).map(|i| (i % 251) as u8).collect();
                assert_eq_result!(fs.write_file_blocks(&mut inode, &first), Ok(()));
                let allocator_at_first = fs.next_block;
                let second: Vec<u8> = (0..(20 * 16)).map(|i| ((i * 7) % 251) as u8).collect();
                assert_eq_result!(fs.write_file_blocks(&mut inode, &second), Ok(()));
                assert_eq_result!(fs.next_block, allocator_at_first, "no new blocks allocated on rewrite");
                assert_eq_result!(fs.read_file_data(&inode).map_err(|_| "read failed")?, second, "rewritten data round-trips");
                Ok(())
            }),
        },
        Test {
            name: "ext2_write_file_blocks_shrink_frees_excess",
            category: "kernel::fs",
            run: Box::new(|| {
                // Shrinking 20 blocks -> 5 frees the excess DATA blocks and
                // zeroes the now-unused direct pointers (indirect chains stay
                // behind per the kernel's own ponytail TODO); the new data
                // round-trips and the old tail is gone.
                let mut fs = IndirectFs::new(16);
                let mut inode = IndirectInode::new();
                let big: Vec<u8> = (0..(20 * 16)).map(|i| (i % 251) as u8).collect();
                assert_eq_result!(fs.write_file_blocks(&mut inode, &big), Ok(()));
                let fifth = inode.i_block[5];
                assert_eq_result!(fs.blocks.contains_key(&fifth), true, "fifth data block exists before shrink");
                let small: Vec<u8> = (0..(5 * 16)).map(|i| (i % 251) as u8).collect();
                assert_eq_result!(fs.write_file_blocks(&mut inode, &small), Ok(()));
                assert_eq_result!(inode.i_size_lo, 5 * 16);
                assert_eq_result!(inode.i_block[5], 0, "now-unused direct pointer zeroed");
                assert_eq_result!(fs.blocks.contains_key(&fifth), false, "excess data block freed");
                assert_eq_result!(fs.read_file_data(&inode).map_err(|_| "read failed")?, small, "shrunk data round-trips");
                Ok(())
            }),
        },
        Test {
            name: "ext2_write_file_blocks_empty_truncates",
            category: "kernel::fs",
            run: Box::new(|| {
                // Writing empty data truncates: needed=0, all data blocks
                // freed, size fields zeroed, direct pointers cleared.
                let mut fs = IndirectFs::new(16);
                let mut inode = IndirectInode::new();
                let data: Vec<u8> = (0..(10 * 16)).map(|i| (i % 251) as u8).collect();
                assert_eq_result!(fs.write_file_blocks(&mut inode, &data), Ok(()));
                let first = inode.i_block[0];
                assert_eq_result!(fs.write_file_blocks(&mut inode, &[]), Ok(()));
                assert_eq_result!(inode.i_size_lo, 0);
                assert_eq_result!(inode.i_blocks_lo, 0);
                assert_eq_result!(inode.i_block[0], 0, "direct pointer cleared on truncate");
                assert_eq_result!(fs.blocks.contains_key(&first), false, "data block freed on truncate");
                assert_eq_result!(fs.read_file_data(&inode).map_err(|_| "read failed")?, Vec::<u8>::new(), "empty file reads empty");
                Ok(())
            }),
        },
    ]
}
