use std::{
    ops::Range,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

#[derive(Copy, Clone, Default, PartialEq)]
pub struct TileId(u64);

impl TileId {
    #[inline(always)]
    pub fn new(tex_id: u32, level: u32, tile_x: u32, tile_y: u32) -> Self {
        let mut bits: u64 = tex_id as u64;
        bits |= (level as u64 & 0xffff) << 16;
        bits |= (tile_x as u64) << 32;
        bits |= (tile_y as u64) << 48;
        Self(bits)
    }

    #[inline(always)]
    pub fn tex_id(&self) -> u32 {
        (self.0 & 0xffff) as u32
    }

    #[inline(always)]
    pub fn level(&self) -> u32 {
        ((self.0 >> 16) & 0xffff) as u32
    }

    #[inline(always)]
    pub fn tile_x(&self) -> u32 {
        ((self.0 >> 32) & 0xffff) as u32
    }

    #[inline(always)]
    pub fn tile_y(&self) -> u32 {
        ((self.0 >> 48) & 0xffff) as u32
    }

    #[inline(always)]
    fn hash(&self) -> u64 {
        let v = self.0;
        let v = v ^ (v >> 31);
        let v = v.wrapping_mul(0x7fb5d329728ea185);
        let v = v ^ (v >> 27);
        let v = v.wrapping_mul(0x81dadef4bc2dd44d);
        let v = v ^ (v >> 33);
        v
    }
}

impl std::fmt::Debug for TileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "TileId(tex:{}, lv:{}, x:{}, y:{})",
            self.tex_id(),
            self.level(),
            self.tile_x(),
            self.tile_y(),
        ))
    }
}

#[derive(Debug)]
pub struct TextureTile {
    pub(crate) index: usize,
    pub(crate) tile_id: TileId,
    pub(crate) texels: Range<usize>,
    pub(crate) marked: AtomicBool,
}

pub struct TileHashTable {
    size: usize,
    table: Box<[AtomicPtr<TextureTile>]>,
    hash_entry_copied: Vec<bool>,
}

impl TileHashTable {
    const NULL: *mut TextureTile = std::ptr::null_mut();

    pub fn new(size: usize) -> Self {
        Self {
            size,
            table: (0..size).map(|_| AtomicPtr::default()).collect::<Box<_>>(),
            hash_entry_copied: vec![false; size],
        }
    }

    pub fn insert(&self, tile: &mut TextureTile) {
        let mut hash_offset = tile.tile_id.hash() % self.size as u64;
        let mut step = 1;
        let mut current = Self::NULL;
        loop {
            // attempt to insert tile at hash_offset
            match self.table[hash_offset as usize].compare_exchange_weak(
                current,
                tile as *mut TextureTile,
                Ordering::Release,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(ptr) => {
                    // handle compare-exchange failure for hash table insertion
                    if ptr != Self::NULL {
                        hash_offset += step * step;
                        step += 1;
                        if hash_offset >= self.size as u64 {
                            hash_offset %= self.size as u64;
                        }
                    }
                    current = ptr;
                }
            }
        }
    }

    pub fn look_up(&self, tile_id: TileId) -> Option<Range<usize>> {
        let mut hash_offset = tile_id.hash() % self.size as u64;
        let mut step = 1;
        loop {
            match self.table[hash_offset as usize].load(Ordering::Acquire) {
                Self::NULL => return None,
                ptr => {
                    let entry = unsafe { &*ptr };
                    if entry.tile_id == tile_id {
                        // update entry's marked field after cache hit
                        if entry.marked.load(Ordering::Relaxed) {
                            entry.marked.store(false, Ordering::Relaxed);
                        }
                        return Some(entry.texels.clone());
                    }
                    // resolve hash collision
                    hash_offset += step * step;
                    step += 1;
                    if hash_offset >= self.size as u64 {
                        hash_offset %= self.size as u64;
                    }
                }
            }
        }
    }

    // Goes through the hash table and sets each tile's marked field to true.
    pub fn mark_entries(&self) {
        for entry in &self.table {
            let entry = entry.load(Ordering::Acquire);
            if entry.is_null() {
                continue;
            }
            let tile = unsafe { &*entry };
            tile.marked.store(true, Ordering::Relaxed);
        }
    }

    // Copies a subset of the tiles to the new hash table, leaving a number of others to be freed.
    pub fn copy_active(&mut self, dest: &mut TileHashTable) {
        let mut n_copied = 0;
        let mut n_active = 0;
        // insert unmarked entries from hash table to dest
        for i in 0..self.size {
            self.hash_entry_copied[i] = false;
            let entry = match self.table[i].load(Ordering::Acquire) {
                Self::NULL => continue,
                ptr => unsafe { &mut *ptr },
            };
            n_active += 1;
            // add entry to dest if unmarked
            if !entry.marked.load(Ordering::Relaxed) {
                self.hash_entry_copied[i] = true;
                n_copied += 1;
                dest.insert(entry);
            }
        }
        // eprintln!("Copied {}/{} tiles during free_tiles", n_copied, n_active);
        // handle case of all entries copied to free_hash_table
        if n_copied == n_active {
            for i in 0..self.size / 2 {
                self.hash_entry_copied[i] = false;
            }
        }
    }

    pub fn reclaim_uncopied(&self, returned: &mut Vec<usize>) {
        for i in 0..self.size {
            let entry = match self.table[i].load(Ordering::Acquire) {
                Self::NULL => continue,
                ptr => unsafe { &mut *ptr },
            };
            if !self.hash_entry_copied[i] {
                returned.push(entry.index);
            }
            self.table[i].store(Self::NULL, Ordering::Relaxed);
        }
    }
}
