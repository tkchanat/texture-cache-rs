mod file_descriptor;
mod hash_table;
mod image;
mod texel;

use hash_table::{TextureTile, TileHashTable};
pub use image::{PowerOfTwo, TiledImage, WrapMode};
use std::{
    os::unix::fs::FileExt,
    pin::Pin,
    sync::{Condvar, Mutex},
};
pub use texel::PixelFormat;

use crate::{file_descriptor::FdCache, hash_table::TileId, texel::Texel};

pub struct TextureCache {
    tile_mem_alloc: Pin<Box<[u8]>>,
    all_tiles_alloc: Box<[TextureTile]>,
    free_tiles: Mutex<Vec<*mut TextureTile>>,
    hash_table: crossbeam_epoch::Atomic<TileHashTable>,
    free_hash_table: TileHashTable,
    textures: Vec<TiledImage>,
    outstanding_reads: Mutex<Vec<TileId>>,
    outstanding_reads_condvar: Condvar,
    mark_free_capacity: usize,
    fd_cache: Mutex<FdCache>,
    // thread_active_flags: crossbeam_epoch::Atomic<Vec<CachePadded<AtomicBool>>>,
}

impl TextureCache {
    const TILE_ALLOC_SIZE: usize = 3 * 64 * 64;
    const MAX_OPENED_FILES: usize = 1024;

    pub fn new(max_memory_mb: usize) -> Self {
        let max_texture_bytes = max_memory_mb * 1_048_576;
        let n_tiles = max_texture_bytes / Self::TILE_ALLOC_SIZE;

        // Allocate tile memory for texture cache
        let tile_mem_alloc =
            Pin::new(vec![0u8; n_tiles * Self::TILE_ALLOC_SIZE].into_boxed_slice());

        // Allocate TextureTiles and initialize free list
        let all_tiles_alloc = (0..n_tiles)
            .map(|_| TextureTile::default())
            .collect::<Box<_>>();
        let free_tiles = Mutex::new(
            (0..n_tiles)
                .map(
                    |i| unsafe { tile_mem_alloc.as_ptr().add(i * Self::TILE_ALLOC_SIZE) }
                        as *mut TextureTile,
                )
                .collect::<Vec<_>>(),
        );
        let hash_size = 8 * n_tiles;
        let mark_free_capacity = 1 + n_tiles / 8;

        Self {
            tile_mem_alloc,
            all_tiles_alloc,
            free_tiles,
            hash_table: crossbeam_epoch::Atomic::new(TileHashTable::new(hash_size)),
            free_hash_table: TileHashTable::new(hash_size),
            textures: Vec::new(),
            outstanding_reads: Mutex::new(Vec::new()),
            outstanding_reads_condvar: Condvar::new(),
            mark_free_capacity,
            fd_cache: Mutex::new(FdCache::new(Self::MAX_OPENED_FILES)),
            // thread_active_flags: (0..std::thread::available_parallelism().unwrap().get())
            //     .map(|_| CachePadded::new(AtomicBool::new(false)))
            //     .collect(),
        }
    }

    pub fn add_texture<P: AsRef<std::path::Path>>(
        &mut self,
        path: P,
    ) -> Result<u32, std::io::Error> {
        // return preexisting id if texture has already been added
        if let Some(tex_id) = self
            .textures
            .iter()
            .rposition(|tex| tex.path == path.as_ref())
        {
            return Ok(tex_id as u32);
        }
        let tex = TiledImage::open(path)?;
        let tex_id = self.textures.len() as u32;
        self.textures.push(tex);
        Ok(tex_id)
    }

    pub fn texel<T: Texel>(&self, tex_id: u32, level: u32, x: u32, y: u32) -> T {
        let tex = &self.textures[tex_id as usize];
        // return texel from preloaded levels, if applicable
        if let Some(bytes) = tex.get_texel(x, y, level) {
            return T::from_bytes(bytes);
        }
        // get texel pointer from cache and return value
        let (tile_x, tile_y) = tex.tile_index(x, y);
        let tile_id = TileId::new(tex_id, level, tile_x, tile_y);
        let texel_offset = tex.texel_offset(x, y);
        let texel = unsafe { self.get_tile(tile_id).add(texel_offset) };
        let bytes = unsafe { std::slice::from_raw_parts(texel, tex.format.texel_bytes()) };
        T::from_bytes(bytes);
        todo!()
    }

    fn read_tile(&self, tile_id: TileId, tile: &mut TextureTile) {
        tile.tile_id = tile_id;
        let tex = &self.textures[tile_id.tex_id() as usize];
        // get file descriptor and seek to start of texture tile
        let offset = tex.file_offset(tile_id.tile_x(), tile_id.tile_y(), tile_id.level());
        let mut fd_cache = self.fd_cache.lock().unwrap();
        let fd_entry = fd_cache.look_up(tile_id.tex_id(), &tex.path);
        let tile_bytes = tex.tile_bytes();
        let buf = unsafe { std::slice::from_raw_parts_mut(tile.texels, tile_bytes) };
        fd_entry
            .read_exact_at(buf, offset as u64)
            .expect("Read error while read_tile");
        // read texel data and return file descriptor
        fd_cache.recycle(fd_entry);
    }

    // Returns a pointer to the start of the texels for the given texture tile.
    fn get_tile(&self, tile_id: TileId) -> *const u8 {
        // return tile if it's present in the hash table
        let guard = crossbeam_epoch::pin();
        let table = unsafe {
            self.hash_table
                .load(std::sync::atomic::Ordering::Acquire, &guard)
                .deref()
        };
        if let Some(bytes) = table.look_up(tile_id) {
            return bytes;
        }
        std::mem::drop(guard);
        // check to see if another thread is already loading this tile
        let mut outstanding_reads = self.outstanding_reads.lock().unwrap();
        for read_tile_id in outstanding_reads.iter() {
            if *read_tile_id == tile_id {
                // wait for tile_id to be read before retrying lookup
                self.outstanding_reads_condvar.wait(outstanding_reads);
                return self.get_tile(tile_id);
            }
        }
        // record that the current thread will read tile_id
        outstanding_reads.push(tile_id);
        std::mem::drop(outstanding_reads);
        // load texture tile from disk
        let tile = self.get_free_tile();
        self.read_tile(tile_id, tile);
        // add tile to hash table and return texel pointer
        let guard = crossbeam_epoch::pin();
        let table = unsafe {
            self.hash_table
                .load(std::sync::atomic::Ordering::Relaxed, &guard)
                .deref()
        };
        table.insert(tile);
        // update outstanding_reads for read tile
        let mut outstanding_reads = self.outstanding_reads.lock().unwrap();
        if let Some(index) = outstanding_reads.iter().position(|x| *x == tile_id) {
            outstanding_reads.swap_remove(index);
        }
        std::mem::drop(outstanding_reads);
        self.outstanding_reads_condvar.notify_all();
        tile.texels
    }

    fn free_tiles(&self) {
        todo!()
    }

    // Returns an available tile, freeing tiles if needed.
    fn get_free_tile(&self) -> &mut TextureTile {
        let mut free_tiles = self.free_tiles.lock().unwrap();
        if free_tiles.is_empty() {
            self.free_tiles();
        }
        // mark hash table entries if free-tile availability is low
        if free_tiles.len() == self.mark_free_capacity {
            let guard = crossbeam_epoch::pin();
            let hash_table = unsafe {
                self.hash_table
                    .load(std::sync::atomic::Ordering::Acquire, &guard)
                    .deref()
            };
            hash_table.mark_entries();
        }
        // return tile from free_tiles
        let tile = unsafe { &mut *free_tiles.pop().unwrap() };
        *tile = TextureTile::default();
        tile
    }
}
