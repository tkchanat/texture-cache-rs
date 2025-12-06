mod file_descriptor;
mod hash_table;
mod image;
mod texel;

use hash_table::{TextureTile, TileHashTable};
pub use image::{PowerOfTwo, TiledImage, WrapMode};
use seize::Guard;
use std::{
    io::{Read, Seek},
    sync::{
        Condvar, Mutex,
        atomic::{AtomicPtr, Ordering},
    },
};
pub use texel::{Luma, PixelFormat, Rgb, Rgba};

use crate::{file_descriptor::FdCache, hash_table::TileId, texel::Texel};

pub struct TextureCache {
    _tile_mem_alloc: Box<[u8]>,
    _all_tiles_alloc: Box<[TextureTile]>,
    free_tiles: Mutex<Vec<usize>>,
    rcu_collector: seize::Collector,
    hash_table: AtomicPtr<TileHashTable>,
    free_hash_table: AtomicPtr<TileHashTable>,
    textures: Vec<TiledImage>,
    outstanding_reads: Mutex<Vec<TileId>>,
    outstanding_reads_condvar: Condvar,
    mark_free_capacity: usize,
    fd_cache: Mutex<FdCache>,
}

impl TextureCache {
    // Just enough to store 64x64 texels for the common 3-channel RGB, 8-bit texture format.
    const TILE_ALLOC_SIZE: usize = 3 * 64 * 64;
    const MAX_OPENED_FILES: usize = 1024;

    pub fn new(max_memory_mb: usize) -> Self {
        let max_texture_bytes = max_memory_mb * 1_048_576;
        let n_tiles = max_texture_bytes / Self::TILE_ALLOC_SIZE;

        // Allocate tile memory for texture cache
        let mut _tile_mem_alloc = vec![0u8; n_tiles * Self::TILE_ALLOC_SIZE].into_boxed_slice();

        // Allocate TextureTiles and initialize free list
        let mut _all_tiles_alloc = (0..n_tiles)
            .map(|i| TextureTile {
                index: i,
                tile_id: TileId::default(),
                offset: i * Self::TILE_ALLOC_SIZE,
                marked: Default::default(),
            })
            .collect::<Box<_>>();
        let free_tiles = Mutex::new((0..n_tiles).collect::<Vec<_>>());
        let hash_size = 8 * n_tiles;
        let mark_free_capacity = 1 + n_tiles / 8;

        Self {
            _tile_mem_alloc,
            _all_tiles_alloc,
            free_tiles,
            rcu_collector: seize::Collector::new(),
            hash_table: AtomicPtr::new(Box::into_raw(Box::new(TileHashTable::new(hash_size)))),
            free_hash_table: AtomicPtr::new(Box::into_raw(Box::new(TileHashTable::new(hash_size)))),
            textures: Vec::new(),
            outstanding_reads: Mutex::new(Vec::new()),
            outstanding_reads_condvar: Condvar::new(),
            mark_free_capacity,
            fd_cache: Mutex::new(FdCache::new(Self::MAX_OPENED_FILES)),
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
            return T::from_bytes(&bytes[0..T::STRIDE]);
        }
        // get texel pointer from cache and return value
        let (tile_x, tile_y) = tex.tile_index(x, y);
        let tile_id = TileId::new(tex_id, level, tile_x, tile_y);
        let tile_offset = self.get_tile(tile_id);
        let texel_offset = tile_offset + tex.texel_offset(x, y);
        let texel = &self._tile_mem_alloc[texel_offset..texel_offset + tex.format.texel_bytes()];
        T::from_bytes(&texel[0..T::STRIDE])
    }

    fn read_tile(&self, tile_id: TileId, tile: &mut TextureTile) {
        tile.tile_id = tile_id;
        let tex = &self.textures[tile_id.tex_id() as usize];
        // get file descriptor and seek to start of texture tile
        let offset = tex.file_offset(tile_id.tile_x(), tile_id.tile_y(), tile_id.level());
        let mut fd_cache = self.fd_cache.lock().unwrap();
        let mut fd_entry = fd_cache.look_up(tile_id.tex_id(), &tex.path);
        let tile_bytes = tex.tile_bytes();
        fd_entry
            .seek(std::io::SeekFrom::Start(offset as u64))
            .expect("Invalid offset into tile");
        let buf = unsafe {
            std::slice::from_raw_parts_mut(
                self._tile_mem_alloc.as_ptr().add(tile.offset) as *mut u8,
                tile_bytes,
            )
        };
        let mut total_bytes_read = 0;
        while total_bytes_read < tile_bytes {
            let bytes_read = fd_entry
                .read(&mut buf[total_bytes_read..])
                .expect("Error while reading tile");
            total_bytes_read += bytes_read;
            fd_entry
                .seek(std::io::SeekFrom::Current(bytes_read as i64))
                .expect("Invalid offset into tile");
        }
        assert_eq!(total_bytes_read, tile_bytes);
        // read texel data and return file descriptor
        fd_cache.recycle(fd_entry);
    }

    // Returns a pointer to the start of the texels for the given texture tile.
    fn get_tile(&self, tile_id: TileId) -> usize {
        // return tile if it's present in the hash table
        let rcu = self.rcu_collector.enter();
        let table = unsafe { &*rcu.protect(&self.hash_table, Ordering::Acquire) };
        if let Some(bytes) = table.look_up(tile_id) {
            return bytes;
        }
        drop(rcu);
        // check to see if another thread is already loading this tile
        let mut outstanding_reads = self.outstanding_reads.lock().unwrap();
        for read_tile_id in outstanding_reads.iter() {
            if *read_tile_id == tile_id {
                // wait for tile_id to be read before retrying lookup
                let outstanding_reads = self
                    .outstanding_reads_condvar
                    .wait(outstanding_reads)
                    .unwrap();
                drop(outstanding_reads);
                return self.get_tile(tile_id);
            }
        }
        // record that the current thread will read tile_id
        outstanding_reads.push(tile_id);
        drop(outstanding_reads);
        // load texture tile from disk
        let tile = self.get_free_tile();
        self.read_tile(tile_id, tile);
        // add tile to hash table and return texel pointer
        let rcu = self.rcu_collector.enter();
        let table = unsafe { &*rcu.protect(&self.hash_table, Ordering::Relaxed) };
        table.insert(tile);
        drop(rcu);
        // update outstanding_reads for read tile
        let mut outstanding_reads = self.outstanding_reads.lock().unwrap();
        if let Some(index) = outstanding_reads.iter().position(|x| *x == tile_id) {
            outstanding_reads.swap_remove(index);
        }
        drop(outstanding_reads);
        self.outstanding_reads_condvar.notify_all();
        tile.offset
    }

    // Returns an available tile, freeing tiles if needed.
    fn get_free_tile(&self) -> &mut TextureTile {
        let mut free_tiles = self.free_tiles.lock().unwrap();
        if free_tiles.is_empty() {
            // copy unmarked tiles to free_hash_table
            let guard = self.rcu_collector.enter();
            let hash_table = unsafe { &mut *guard.protect(&self.hash_table, Ordering::Relaxed) };
            let free_hash_table_ptr = guard.protect(&self.free_hash_table, Ordering::Relaxed);
            let free_hash_table = unsafe { &*free_hash_table_ptr };
            hash_table.copy_active(free_hash_table);
            // swap texture cache hash tables
            self.free_hash_table.store(
                self.hash_table.swap(free_hash_table_ptr, Ordering::AcqRel),
                Ordering::Relaxed,
            );
            // TODO: ensure that no threads are accessing the old hash table
            // add inactive tiles in free_hash_table to free list
            free_hash_table.reclaim_uncopied(&mut free_tiles);
            if free_tiles.len() < self.mark_free_capacity {
                hash_table.mark_entries();
            }
        }
        assert!(!free_tiles.is_empty());
        // mark hash table entries if free-tile availability is low
        if free_tiles.len() == self.mark_free_capacity {
            let guard = self.rcu_collector.enter();
            let hash_table = unsafe { &*guard.protect(&self.hash_table, Ordering::Acquire) };
            hash_table.mark_entries();
        }
        // return tile from free_tiles
        unsafe {
            self._all_tiles_alloc
                .as_ptr()
                .add(free_tiles.pop().unwrap())
                .cast_mut()
                .as_mut()
                .unwrap()
        }
    }
}

impl Drop for TextureCache {
    fn drop(&mut self) {
        unsafe {
            drop(Box::from_raw(self.hash_table.load(Ordering::Relaxed)));
            drop(Box::from_raw(self.free_hash_table.load(Ordering::Relaxed)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::image::{ImageBuffer, ImageFormat, Rgb};
    use rayon::iter::ParallelIterator;

    #[test]
    fn test_texture_cache() {
        let mut cache = TextureCache::new(1);
        let tex_id = cache
            .add_texture("assets/checker.txp")
            .expect("Failed to add texture");

        let mut out: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::new(1024, 1024);
        let level = 0;
        out.par_enumerate_pixels_mut().for_each(|(x, y, pixel)| {
            let texel: texel::Rgb<u8> = cache.texel(tex_id, level, x, y);
            let pixel = &mut pixel.0;
            *pixel = texel.as_ref().clone();
        });
        let mut out_file = std::fs::File::create("assets/test.png").unwrap();
        out.write_to(&mut out_file, ImageFormat::Png).unwrap();
    }
}
