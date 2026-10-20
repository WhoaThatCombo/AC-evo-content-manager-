//! dev: BC7 basic vs fast on one image
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let img = aceconv::io::texture::decode_any(&std::fs::read(&a[1]).unwrap()).unwrap();
    println!("{}x{}", img.w, img.h);
    let s = intel_tex_2::RgbaSurface { data: &img.px, width: img.w / 4 * 4, height: img.h / 4 * 4, stride: img.w * 4 };
    use rayon::prelude::*;
    for (name, set) in [("basic", intel_tex_2::bc7::opaque_basic_settings()), ("fast", intel_tex_2::bc7::opaque_fast_settings()), ("very_fast", intel_tex_2::bc7::opaque_very_fast_settings())] {
        let t = Instant::now();
        let rows: Vec<u32> = (0..s.height).step_by(64).collect();
        let n: usize = rows.par_iter().map(|&y| {
            let h = 64.min(s.height - y);
            let sub = intel_tex_2::RgbaSurface { data: &img.px[(y * s.stride) as usize..], width: s.width, height: h, stride: s.stride };
            intel_tex_2::bc7::compress_blocks(&set, &sub)
        }).flatten().collect::<Vec<u8>>().len();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        // PSNR vs the source over the whole image
        let blocks = intel_tex_2::bc7::compress_blocks(&set, &s);
        let mut words = vec![0u32; (s.width * s.height) as usize];
        texture2ddecoder::decode_bc7(&blocks, s.width as usize, s.height as usize, &mut words).unwrap();
        let mut se = 0f64;
        for y in 0..s.height { for x in 0..s.width {
            let wv = words[(y * s.width + x) as usize];
            let dec = [(wv >> 16) as u8, (wv >> 8) as u8, wv as u8];
            let o = ((y * img.w + x) * 4) as usize;
            for c in 0..3 { let d = dec[c] as f64 - img.px[o + c] as f64; se += d * d; }
        }}
        let mse = se / (s.width as f64 * s.height as f64 * 3.0);
        println!("{name:10} {ms:.0} ms ({n} bytes) PSNR {:.2} dB", 10.0 * (255.0f64 * 255.0 / mse).log10());
    }
}
