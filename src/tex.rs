use anyhow::{Context, Result, anyhow, bail};
use ironworks::file::tex::Format;
use texture2ddecoder::{decode_bc1, decode_bc3, decode_bc7};
use webp::Encoder;

use crate::config::WEBP_QUALITY;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct TextureHeader {
	attribute: u32,
	format: u32,
	width: u16,
	height: u16,
	depth: u16,
	mip_levels: u8,
	array_size: u8,
	lod_offsets: [u32; 3],
	surface_offsets: [u32; 13],
}

impl TextureHeader {
	const SIZE: usize = 0x50;

	pub fn parse(data: &[u8]) -> Result<Self> {
		if data.len() < Self::SIZE {
			bail!(
				"invalid texture buffer: expected at least {} bytes, got {}",
				Self::SIZE,
				data.len()
			);
		}

		let read_u32 = |offset| {
			u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
		};
		let read_u16 = |offset| {
			u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
		};

		let mut lod_offsets = [0u32; 3];
		for (index, entry) in lod_offsets.iter_mut().enumerate() {
			*entry = read_u32(16 + index * 4);
		}

		let mut surface_offsets = [0u32; 13];
		for (index, entry) in surface_offsets.iter_mut().enumerate() {
			*entry = read_u32(28 + index * 4);
		}

		Ok(Self {
			attribute: read_u32(0),
			format: read_u32(4),
			width: read_u16(8),
			height: read_u16(10),
			depth: read_u16(12),
			mip_levels: data[14],
			array_size: data[15],
			lod_offsets,
			surface_offsets,
		})
	}
}

pub fn convert_tex_to_webp(data: &[u8]) -> Result<Vec<u8>> {
	let header = TextureHeader::parse(data)?;
	let rgba = decode_tex_rgba(data, header)?;
	let webp = Encoder::from_rgba(&rgba, u32::from(header.width), u32::from(header.height))
		.encode(WEBP_QUALITY);
	Ok(webp.to_vec())
}

fn decode_tex_rgba(data: &[u8], header: TextureHeader) -> Result<Vec<u8>> {
	match header.format {
		x if x == Format::Bgra8Unorm as u32 => decode_bgra8(data, header, true),
		x if x == Format::Bgrx8Unorm as u32 => decode_bgra8(data, header, false),
		x if x == Format::Bgra4Unorm as u32 => decode_bgra4(data, header),
		x if x == Format::Bgr5a1Unorm as u32 => decode_bgr5a1(data, header),
		x if x == Format::Bc1Unorm as u32 => decode_block_compressed(data, header, 8, decode_bc1),
		x if x == Format::Bc3Unorm as u32 => {
			decode_block_compressed(data, header, 16, decode_bc3)
		}
		x if x == Format::Bc7Unorm as u32 => {
			decode_block_compressed(data, header, 16, decode_bc7)
		}
		other => bail!("unsupported texture format 0x{other:04x}"),
	}
}

fn decode_bgra8(data: &[u8], header: TextureHeader, preserve_alpha: bool) -> Result<Vec<u8>> {
	let pixel_count = usize::from(header.width) * usize::from(header.height);
	let source = pixel_data(data, header, pixel_count * 4)?;
	let mut rgba = vec![0; pixel_count * 4];

	for (src, dst) in source.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
		dst[0] = src[2];
		dst[1] = src[1];
		dst[2] = src[0];
		dst[3] = if preserve_alpha { src[3] } else { 0xff };
	}

	Ok(rgba)
}

fn decode_bgra4(data: &[u8], header: TextureHeader) -> Result<Vec<u8>> {
	let pixel_count = usize::from(header.width) * usize::from(header.height);
	let source = pixel_data(data, header, pixel_count * 2)?;
	let mut rgba = vec![0; pixel_count * 4];

	for (src, dst) in source.chunks_exact(2).zip(rgba.chunks_exact_mut(4)) {
		let packed = u16::from_le_bytes([src[0], src[1]]);
		dst[0] = scale_4bit(((packed >> 8) & 0x0f) as u8);
		dst[1] = scale_4bit(((packed >> 4) & 0x0f) as u8);
		dst[2] = scale_4bit((packed & 0x0f) as u8);
		dst[3] = scale_4bit(((packed >> 12) & 0x0f) as u8);
	}

	Ok(rgba)
}

fn decode_bgr5a1(data: &[u8], header: TextureHeader) -> Result<Vec<u8>> {
	let pixel_count = usize::from(header.width) * usize::from(header.height);
	let source = pixel_data(data, header, pixel_count * 2)?;
	let mut rgba = vec![0; pixel_count * 4];

	for (src, dst) in source.chunks_exact(2).zip(rgba.chunks_exact_mut(4)) {
		let packed = u16::from_le_bytes([src[0], src[1]]);
		dst[0] = scale_5bit(((packed >> 10) & 0x1f) as u8);
		dst[1] = scale_5bit(((packed >> 5) & 0x1f) as u8);
		dst[2] = scale_5bit((packed & 0x1f) as u8);
		dst[3] = if (packed & 0x8000) != 0 { 0xff } else { 0x00 };
	}

	Ok(rgba)
}

fn decode_block_compressed(
	data: &[u8],
	header: TextureHeader,
	bytes_per_block: usize,
	decoder: fn(&[u8], usize, usize, &mut [u32]) -> std::result::Result<(), &'static str>,
) -> Result<Vec<u8>> {
	let width = usize::from(header.width);
	let height = usize::from(header.height);
	let block_width = width.div_ceil(4).max(1);
	let block_height = height.div_ceil(4).max(1);
	let compressed = pixel_data(data, header, block_width * block_height * bytes_per_block)?;
	let mut pixels = vec![0u32; width * height];

	decoder(compressed, width, height, &mut pixels)
		.map_err(|error| anyhow!("texture decode failed: {error}"))?;

	let mut rgba = vec![0u8; width * height * 4];
	for (pixel, dst) in pixels.into_iter().zip(rgba.chunks_exact_mut(4)) {
		let [b, g, r, a] = pixel.to_le_bytes();
		dst[0] = r;
		dst[1] = g;
		dst[2] = b;
		dst[3] = a;
	}

	Ok(rgba)
}

fn pixel_data<'a>(data: &'a [u8], header: TextureHeader, length: usize) -> Result<&'a [u8]> {
	let start = usize::try_from(header.surface_offsets[0]).context("invalid surface offset")?;
	let end = start + length;
	if start < TextureHeader::SIZE || end > data.len() {
		bail!(
			"texture pixel data out of bounds: {}..{} of {}",
			start,
			end,
			data.len()
		);
	}

	Ok(&data[start..end])
}

fn scale_4bit(value: u8) -> u8 {
	(value << 4) | value
}

fn scale_5bit(value: u8) -> u8 {
	(value << 3) | (value >> 2)
}
