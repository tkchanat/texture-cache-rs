use criterion::*;
use texture_cache_rs::{Rgb, TextureCache};

fn hashtable_benchmark(c: &mut Criterion) {
    let mut cache = TextureCache::new(64);
    let tex_id = cache.add_texture("assets/checker.txp").unwrap();
    let level = 0;
    c.bench_function("TextureCache::texel", |b| {
        b.iter(|| {
            (0..1024 * 1024).for_each(|i| {
                cache.texel::<Rgb<u8>>(tex_id, level, i % 1024, i / 1024);
            });
        })
    });
}

criterion_group!(benches, hashtable_benchmark);
criterion_main!(benches);
