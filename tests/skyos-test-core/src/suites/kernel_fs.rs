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
    ]
}
