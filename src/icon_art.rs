// Shared between build.rs (to make the .ico) and the app (window icon), so it has no crate deps.

/// RGBA pixels of the Plainly icon: a lens (ring + dot) on a teal-to-blue rounded square.
pub fn render(size: u32) -> Vec<u8> {
    let s = size as f32;
    let half = s / 2.0;
    let corner = s * 0.22;
    let ring_radius = s * 0.25;
    let ring_half_width = s * 0.06;
    let dot_radius = s * 0.10;
    let from = [0x0f as f32, 0x76 as f32, 0x6e as f32];
    let to = [0x25 as f32, 0x63 as f32, 0xeb as f32];

    let mut px = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;

            let qx = ((fx - half).abs() - (half - corner)).max(0.0);
            let qy = ((fy - half).abs() - (half - corner)).max(0.0);
            let edge = (qx * qx + qy * qy).sqrt() - corner;
            let bg_alpha = (0.5 - edge).clamp(0.0, 1.0);
            if bg_alpha <= 0.0 {
                continue;
            }

            let t = ((fx + fy) / (2.0 * s)).clamp(0.0, 1.0);
            let mut rgb = [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
                from[2] + (to[2] - from[2]) * t,
            ];

            let dist = ((fx - half).powi(2) + (fy - half).powi(2)).sqrt();
            let ring = (ring_half_width - (dist - ring_radius).abs() + 0.5).clamp(0.0, 1.0);
            let dot = (dot_radius - dist + 0.5).clamp(0.0, 1.0);
            let white = ring.max(dot);
            for c in rgb.iter_mut() {
                *c += (255.0 - *c) * white;
            }

            let i = ((y * size + x) * 4) as usize;
            px[i] = rgb[0] as u8;
            px[i + 1] = rgb[1] as u8;
            px[i + 2] = rgb[2] as u8;
            px[i + 3] = (bg_alpha * 255.0) as u8;
        }
    }
    px
}
