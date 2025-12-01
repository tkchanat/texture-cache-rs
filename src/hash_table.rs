use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

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
        let v = v * 0x7fb5d329728ea185;
        let v = v ^ (v >> 27);
        let v = v * 0x81dadef4bc2dd44d;
        let v = v ^ (v >> 33);
        v
    }
}

#[derive(Default)]
pub struct TextureTile {
    pub(crate) tile_id: TileId,
    pub(crate) texels: *mut u8,
    pub(crate) marked: AtomicBool,
}

pub struct TileHashTable {
    table: Box<[AtomicPtr<TextureTile>]>,
    size: u64,
}

impl TileHashTable {
    const NULL: *mut TextureTile = std::ptr::null_mut();

    pub fn new(size: usize) -> Self {
        Self {
            table: (0..size).map(|_| AtomicPtr::default()).collect::<Box<_>>(),
            size: size as u64,
        }
    }

    pub fn insert(&self, tile: &mut TextureTile) {
        let mut hash_offset = tile.tile_id.hash().rem_euclid(self.size);
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
                        if hash_offset >= self.size {
                            hash_offset &= self.size;
                        }
                    }
                    current = ptr;
                }
            }
        }
    }

    pub fn look_up(&self, tile_id: TileId) -> Option<*const u8> {
        let mut hash_offset = tile_id.hash().rem_euclid(self.size);
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
                        return Some(entry.texels);
                    }
                    // resolve hash collision
                    hash_offset += step * step;
                    step += 1;
                    if hash_offset >= self.size {
                        hash_offset %= self.size;
                    }
                }
            }
        }
    }

    // Goes through the hash table and sets each tile's marked field to true.
    pub fn mark_entries(&self) {
        for entry in &self.table {
            let tile = unsafe { &*entry.load(Ordering::Acquire) };
            tile.marked.store(true, Ordering::Relaxed);
        }
    }
}
