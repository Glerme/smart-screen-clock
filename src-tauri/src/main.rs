#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::{imageops, AnimationDecoder, Frame};
use std::io::Cursor;
use tauri::ipc::{InvokeBody, Request, Response};

/// Corta o centro do GIF em quadrado e redimensiona para `size` x `size`, mantendo a animação.
fn crop_gif_square(bytes: &[u8], size: u32) -> Result<Vec<u8>, String> {
    let frames = GifDecoder::new(Cursor::new(bytes))
        .and_then(|d| d.into_frames().collect_frames())
        .map_err(|e| format!("GIF inválido: {e}"))?;
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new_with_speed(&mut out, 10);
        enc.set_repeat(Repeat::Infinite).map_err(|e| e.to_string())?;
        for f in frames {
            let delay = f.delay();
            let mut buf = f.into_buffer();
            let (w, h) = buf.dimensions();
            let s = w.min(h);
            let sq = imageops::crop(&mut buf, (w - s) / 2, (h - s) / 2, s, s).to_image();
            let small = imageops::resize(&sq, size, size, imageops::FilterType::Triangle);
            enc.encode_frame(Frame::from_parts(small, 0, 0, delay))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(out)
}

#[tauri::command]
fn crop_gif(request: Request) -> Result<Response, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("esperava os bytes do GIF".into());
    };
    // Tamanho vem no cabeçalho "size": 80 para a animação do relógio, 240 para a tela Photo
    let size = match request.headers().get("size").and_then(|v| v.to_str().ok()) {
        Some("240") => 240,
        _ => 80,
    };
    crop_gif_square(bytes, size).map(Response::new)
}

fn main() {
    tauri::Builder::default()
        // HTTP pelo Rust: a tela não envia cabeçalhos CORS, então fetch do webview falharia
        .plugin(tauri_plugin_http::init())
        .invoke_handler(tauri::generate_handler![crop_gif])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o app");
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Delay, RgbaImage};

    #[test]
    fn corta_gif_retangular_para_80x80_mantendo_frames() {
        let mut src = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut src);
            for c in [0u8, 255] {
                let img = RgbaImage::from_pixel(160, 100, image::Rgba([c, 0, 0, 255]));
                enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(100, 1))).unwrap();
            }
        }
        let out = crop_gif_square(&src, 80).unwrap();
        let frames = GifDecoder::new(Cursor::new(out)).unwrap().into_frames().collect_frames().unwrap();
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| f.buffer().dimensions() == (80, 80)));
    }
}
