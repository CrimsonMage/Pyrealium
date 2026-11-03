//! Local testing travel. Derive outdoor feet positions from the same DAT lattice
//! and triangle diagonal as the Unreal renderer (LandscapeDefs / CLandBlockData).
use crate::{dat::Dat, movement::Placement};
use std::{io, path::Path};

pub const CAPABILITY: &str = "[Pyrealium:map-travel-v1]";
fn invalid(text: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, text)
}
fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
pub fn point(words: &[&str]) -> io::Result<[f64; 2]> {
    if words.len() != 3 {
        return Err(invalid("Use @maptravel LAND_X LAND_Y."));
    }
    let x = words[1]
        .parse::<f64>()
        .map_err(|_| invalid("Invalid map coordinates."))?;
    let y = words[2]
        .parse::<f64>()
        .map_err(|_| invalid("Invalid map coordinates."))?;
    if !x.is_finite()
        || !y.is_finite()
        || !(0.0..2040.0).contains(&x)
        || !(0.0..2040.0).contains(&y)
    {
        return Err(invalid("That point is outside the world."));
    }
    Ok([x, y])
}
pub struct Landscape {
    cells: Dat,
    heights: [f32; 256],
    terrain_mask: u32,
}
impl Landscape {
    pub fn load(directory: &Path) -> io::Result<Self> {
        let mut portal = Dat::open(directory.join("portal.dat"))?;
        let bytes = portal.read(0x36000000)?;
        if bytes.len() != 1076
            || word(&bytes, 0) != 0x36000000
            || f32::from_bits(word(&bytes, 4)) != 20.0
            || word(&bytes, 8) != 8
            || word(&bytes, 12) != 2
        {
            return Err(invalid("Unsupported map travel landscape lattice."));
        }
        let heights: [f32; 256] = std::array::from_fn(|i| f32::from_bits(word(&bytes, 52 + i * 4)));
        if heights.iter().any(|v| !v.is_finite() || v.abs() > 100000.0) {
            return Err(invalid("Invalid landscape height table."));
        }
        let bits = word(&bytes, 28);
        if bits == 0 || bits > 32 {
            return Err(invalid("Invalid landscape terrain mask."));
        }
        Ok(Self {
            cells: Dat::open(directory.join("cell_1.dat"))?,
            heights,
            terrain_mask: u32::MAX >> (32 - bits),
        })
    }
    pub fn destination(&mut self, land: [f64; 2], heading: u8) -> io::Result<Placement> {
        if land
            .iter()
            .any(|v| !v.is_finite() || !(0.0..2040.0).contains(v))
        {
            return Err(invalid("That point is outside the world."));
        }
        let block_x = (land[0] / 8.0) as u32;
        let block_y = (land[1] / 8.0) as u32;
        let id = (block_x << 24) | (block_y << 16) | 0xffff;
        let bytes = self
            .cells
            .read(id)
            .map_err(|_| invalid("No terrain exists at that map position."))?;
        let xy = [(land[0] % 8.0 * 20.0) as f32, (land[1] % 8.0 * 20.0) as f32];
        let height = terrain_height(&bytes, id, &self.heights, self.terrain_mask, xy)?;
        let cell = ((xy[0] / 20.0) as u32 * 8 + (xy[1] / 20.0) as u32) + 1;
        Ok(Placement {
            cell: (id & 0xffff0000) | cell,
            coordinates: [xy[0], xy[1], height + 0.05],
            heading,
        })
    }
}
fn terrain_height(
    bytes: &[u8],
    id: u32,
    heights: &[f32; 256],
    mask: u32,
    xy: [f32; 2],
) -> io::Result<f32> {
    if bytes.len() != 1468
        || word(bytes, 0) != id
        || word(bytes, 12) != 289
        || word(bytes, 305) != 289
    {
        return Err(invalid("Unsupported map travel terrain record."));
    }
    let x = (xy[0] / 10.0) as usize;
    let y = (xy[1] / 10.0) as usize;
    if x >= 16 || y >= 16 {
        return Err(invalid("Map position is outside its block."));
    }
    let a = x * 17 + y;
    let indices = [a, a + 17, a + 1, a + 18];
    if indices
        .iter()
        .any(|i| word(bytes, 309 + i * 4) & mask == 0x1e)
    {
        return Err(invalid(
            "That map point is a cave opening. Choose nearby outdoor ground.",
        ));
    }
    let [a, b, c, d] = indices.map(|i| heights[bytes[16 + i] as usize]);
    let gx = (id >> 24) * 16 + x as u32;
    let gy = ((id >> 16) & 255) * 16 + y as u32;
    let seed = gy
        .wrapping_mul(gx)
        .wrapping_mul(gx)
        .wrapping_mul(0x39557fcf)
        .wrapping_sub(0x4a6506db)
        .wrapping_mul(gy)
        .wrapping_sub(gx.wrapping_mul(0x6b3c5f11))
        .wrapping_add(0xae226ceb);
    let u = xy[0] / 10.0 - x as f32;
    let v = xy[1] / 10.0 - y as f32;
    let (height, dx, dy) = if seed & 0x80000000 != 0 {
        if u >= v {
            (a + (b - a) * u + (d - b) * v, b - a, d - b)
        } else {
            (a + (d - c) * u + (c - a) * v, d - c, c - a)
        }
    } else if u + v <= 1.0 {
        (a + (b - a) * u + (c - a) * v, b - a, c - a)
    } else {
        (d + (c - d) * (1.0 - u) + (b - d) * (1.0 - v), d - c, d - b)
    };
    // Match the client's 55-degree walkable floor check; reject a cliff before
    // forcing a move that cannot produce a standing capsule.
    if dx * dx + dy * dy > 100.0 * (55.0f32.to_radians().tan()).powi(2) {
        return Err(invalid(
            "That ground is too steep for map travel. Choose nearby level ground.",
        ));
    }
    Ok(height)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coordinates_reject_invalid_and_nonfinite() {
        for values in [
            ["@maptravel", "NaN", "8"],
            ["@maptravel", "8", "inf"],
            ["@maptravel", "-1", "8"],
            ["@maptravel", "2040", "8"],
        ] {
            assert!(point(&values).is_err());
        }
        assert!(point(&["@maptravel", "1"]).is_err());
        assert_eq!(
            point(&["@maptravel", "692.25", "828.5"]).unwrap(),
            [692.25, 828.5]
        );
    }
    #[test]
    fn terrain_uses_x_major_samples_and_cave_mask() {
        let mut bytes = vec![0; 1468];
        let id = 0x5667ffffu32;
        for (offset, value) in [(0, id), (12, 289), (305, 289)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let heights = std::array::from_fn(|i| i as f32);
        for x in 0..17 {
            for y in 0..17 {
                bytes[16 + x * 17 + y] = (x * 10 + y) as u8;
            }
        }
        assert!(
            (terrain_height(&bytes, id, &heights, 31, [12.5, 27.5]).unwrap() - 15.25).abs() < 0.001
        );
        let offset = 309 + (17 + 2) * 4;
        bytes[offset..offset + 4].copy_from_slice(&0x1eu32.to_le_bytes());
        assert!(terrain_height(&bytes, id, &heights, 31, [12.5, 27.5]).is_err());
        assert!(terrain_height(&bytes[..100], id, &heights, 31, [12.5, 27.5]).is_err());
        bytes[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes());
        let steep = std::array::from_fn(|i| i as f32 * 10.0);
        assert!(terrain_height(&bytes, id, &steep, 31, [12.5, 27.5]).is_err());
    }
}
