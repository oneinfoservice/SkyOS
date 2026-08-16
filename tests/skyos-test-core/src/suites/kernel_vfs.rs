use crate::Test;

/// Host port of the kernel's VFS page cache (`kernel/src/vfs/page_cache.rs`).
///
/// The kernel caches `(inode_id, page_index) -> Page` in a HashMap with
/// single-slot eviction when the cache reaches MAX_CACHED_PAGES (4096); the
/// kernel comment documents the intent as FIFO eviction (hashbrown's
/// `keys().next()` is arbitrary). This port makes that FIFO intent
/// deterministic with an insertion-order queue so the eviction order is
/// testable — same capacity semantics, same single-oldest-evicted rule.
struct Page {
    data: [u8; 4096],
    dirty: bool,
}

struct PageCache {
    pages: std::collections::HashMap<(u64, u64), Page>,
    /// Insertion order of keys, for deterministic FIFO eviction.
    order: std::collections::VecDeque<(u64, u64)>,
    capacity: usize,
}

impl PageCache {
    fn new(capacity: usize) -> Self {
        PageCache {
            pages: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
            capacity,
        }
    }

    fn get_page(&self, ino: u64, index: u64) -> Option<&Page> {
        self.pages.get(&(ino, index))
    }

    fn insert_page(&mut self, ino: u64, index: u64, data: [u8; 4096]) {
        let key = (ino, index);
        let page = Page { data, dirty: false };
        // Existing page: overwrite in place, LRU order untouched.
        if self.pages.insert(key, page).is_some() {
            return;
        }
        // New page: evict the LRU entry once past capacity, then record
        // the insertion in the order queue.
        if self.pages.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.pages.remove(&oldest);
            }
        }
        self.order.push_back(key);
    }

    fn mark_dirty(&mut self, ino: u64, index: u64) {
        if let Some(page) = self.pages.get_mut(&(ino, index)) {
            page.dirty = true;
        }
    }

    fn evict_inode(&mut self, ino: u64) {
        self.order.retain(|(i, _)| *i != ino);
        self.pages.retain(|(i, _), _| *i != ino);
    }
}

pub fn tests() -> Vec<Test> {
    vec![
        Test {
            name: "page_cache_miss_on_absent",
            category: "kernel::vfs",
            run: Box::new(|| {
                let mut cache = PageCache::new(4);
                assert_result!(cache.get_page(1, 0).is_none(), "empty cache has no page");
                let _ = &mut cache;
                Ok(())
            }),
        },
        Test {
            name: "page_cache_hit_after_insert",
            category: "kernel::vfs",
            run: Box::new(|| {
                let mut cache = PageCache::new(4);
                let mut data = [0u8; 4096];
                data[0] = 0xAB;
                data[4095] = 0xCD;
                cache.insert_page(7, 3, data);
                let page = cache.get_page(7, 3);
                assert_result!(page.is_some(), "inserted page must be found");
                let page = page.unwrap();
                assert_eq_result!(page.data[0], 0xAB);
                assert_eq_result!(page.data[4095], 0xCD);
                assert_result!(!page.dirty, "fresh insert is clean");
                Ok(())
            }),
        },
        Test {
            name: "page_cache_evicts_oldest_at_capacity",
            category: "kernel::vfs",
            run: Box::new(|| {
                let mut cache = PageCache::new(4);
                for i in 0..4u64 {
                    cache.insert_page(1, i, [0u8; 4096]);
                }
                // Fifth insert evicts the oldest (index 0), keeps the rest.
                cache.insert_page(1, 4, [0u8; 4096]);
                assert_result!(cache.get_page(1, 0).is_none(), "oldest evicted at capacity");
                for i in 1..5u64 {
                    assert_result!(cache.get_page(1, i).is_some(), "newer pages survive");
                }
                Ok(())
            }),
        },
        Test {
            name: "page_cache_fifo_eviction_order",
            category: "kernel::vfs",
            run: Box::new(|| {
                let mut cache = PageCache::new(4);
                for i in 0..6u64 {
                    cache.insert_page(1, i, [0u8; 4096]);
                }
                // 6 inserts into a 4-slot cache: indexes 0,1 evicted first.
                assert_result!(cache.get_page(1, 0).is_none(), "first inserted evicted");
                assert_result!(cache.get_page(1, 1).is_none(), "second inserted evicted");
                for i in 2..6u64 {
                    assert_result!(cache.get_page(1, i).is_some(), "surviving page {}", i);
                }
                Ok(())
            }),
        },
        Test {
            name: "page_cache_reinsert_replaces",
            category: "kernel::vfs",
            run: Box::new(|| {
                let mut cache = PageCache::new(4);
                let mut a = [0u8; 4096];
                a[0] = 1;
                cache.insert_page(1, 0, a);
                let mut b = [0u8; 4096];
                b[0] = 2;
                cache.insert_page(1, 0, b);
                let page = cache.get_page(1, 0).unwrap();
                assert_eq_result!(page.data[0], 2, "reinsert replaces data");
                // Reinsert must not count as a new slot: 3 more inserts fit.
                for i in 1..4u64 {
                    cache.insert_page(1, i, [0u8; 4096]);
                }
                assert_result!(cache.get_page(1, 0).is_some(), "reinserted key not evicted early");
                Ok(())
            }),
        },
        Test {
            name: "page_cache_evict_inode_selective",
            category: "kernel::vfs",
            run: Box::new(|| {
                let mut cache = PageCache::new(8);
                for ino in 0..2u64 {
                    for i in 0..3u64 {
                        cache.insert_page(ino, i, [0u8; 4096]);
                    }
                }
                cache.evict_inode(0);
                for i in 0..3u64 {
                    assert_result!(cache.get_page(0, i).is_none(), "inode 0 page evicted");
                    assert_result!(cache.get_page(1, i).is_some(), "inode 1 page survives");
                }
                Ok(())
            }),
        },
        Test {
            name: "page_cache_mark_dirty",
            category: "kernel::vfs",
            run: Box::new(|| {
                let mut cache = PageCache::new(4);
                cache.insert_page(5, 2, [0u8; 4096]);
                assert_result!(!cache.get_page(5, 2).unwrap().dirty, "clean before mark");
                cache.mark_dirty(5, 2);
                assert_result!(cache.get_page(5, 2).unwrap().dirty, "dirty after mark");
                // mark_dirty on an absent page is a no-op (no panic).
                cache.mark_dirty(99, 99);
                Ok(())
            }),
        },
    ]
}
