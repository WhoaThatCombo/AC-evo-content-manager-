//! dev: size, mean RGBA and the share of low-alpha texels of image files
//! (DDS / PNG / JPG / TGA, as the converter decodes them).
fn main() {
    for p in std::env::args().skip(1) {
        let b = match std::fs::read(&p) {
            Ok(b) => b,
            Err(e) => {
                println!("{p}: {e}");
                continue;
            }
        };
        match aceconv::io::texture::decode_any(&b) {
            Ok(img) => {
                let m = aceconv::car::materials::mean(&img);
                let n = (img.w * img.h) as f64;
                let low = img.px.chunks_exact(4).filter(|c| c[3] < 128).count() as f64 / n;
                println!("{p}: {}x{} mean rgba {:.3} {:.3} {:.3} {:.3} alpha<0.5 {:.2} var {:.4}", img.w, img.h, m[0], m[1], m[2], m[3], low, aceconv::car::materials::colour_var(&img));
            }
            Err(e) => println!("{p}: {e}"),
        }
    }
}
