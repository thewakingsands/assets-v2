use std::{
	collections::HashSet,
	fs::File,
	io::{BufWriter, Write},
	sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use ironworks::Ironworks;
use serde::Serialize;
use sha2::{Digest, Sha256};
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::{config::VERSIONS, tex::convert_tex_to_webp};

#[derive(Debug, Serialize)]
pub struct MappingEntry {
	path: String,
	sha256: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ExportStats {
	pub found: usize,
	pub converted: usize,
	pub archived: usize,
	pub duplicates: usize,
	pub errors: usize,
}

impl ExportStats {
	pub fn merge(&mut self, other: ExportStats) {
		self.found += other.found;
		self.converted += other.converted;
		self.archived += other.archived;
		self.duplicates += other.duplicates;
		self.errors += other.errors;
	}
}

pub struct ArchiveEntry {
	sha256: String,
	webp: Vec<u8>,
}

pub struct IdOutput {
	pub mappings: Vec<MappingEntry>,
	pub archive_entries: Vec<ArchiveEntry>,
	pub stats: ExportStats,
	pub errors: Vec<String>,
}

pub type Deduper = Arc<Mutex<HashSet<String>>>;

pub fn new_deduper() -> Deduper {
	Arc::new(Mutex::new(HashSet::new()))
}

pub struct ArchiveWriter {
	zip: ZipWriter<BufWriter<File>>,
	zip_options: SimpleFileOptions,
	mappings: Vec<MappingEntry>,
	stats: ExportStats,
}

impl ArchiveWriter {
	pub fn new(zip: ZipWriter<BufWriter<File>>, zip_options: SimpleFileOptions) -> Self {
		Self {
			zip,
			zip_options,
			mappings: Vec::new(),
			stats: ExportStats::default(),
		}
	}

	pub fn write_output(&mut self, output: IdOutput) -> Result<()> {
		for entry in output.archive_entries {
			self.zip
				.start_file(format!("{}.webp", entry.sha256), self.zip_options)
				.with_context(|| format!("start zip entry for {}", entry.sha256))?;
			self.zip
				.write_all(&entry.webp)
				.with_context(|| format!("write zip entry for {}", entry.sha256))?;
		}

		self.mappings.extend(output.mappings);
		self.stats.merge(output.stats);

		for error in output.errors {
			eprintln!("{error}");
		}

		Ok(())
	}

	pub fn stats(&self) -> ExportStats {
		self.stats
	}

	pub fn finish(self) -> Result<(BufWriter<File>, Vec<MappingEntry>, ExportStats)> {
		let mut writer = self.zip.finish().context("finalize zip archive")?;
		writer.flush().context("flush archive output")?;
		Ok((writer, self.mappings, self.stats))
	}
}

pub fn process_id(ironworks: &Ironworks, deduper: &Deduper, id: u32) -> IdOutput {
	let id_string = format!("{id:06}");
	let mut output = IdOutput {
		mappings: Vec::new(),
		archive_entries: Vec::new(),
		stats: ExportStats::default(),
		errors: Vec::new(),
	};

	for version in VERSIONS {
		let base = format!("ui/icon/{}000{version}/{id_string}", &id_string[..3]);
		process_path(ironworks, deduper, &format!("{base}.tex"), &mut output);
		process_path(ironworks, deduper, &format!("{base}_hr1.tex"), &mut output);
	}

	output
}

fn process_path(ironworks: &Ironworks, deduper: &Deduper, path: &str, output: &mut IdOutput) {
	let data = match ironworks.file::<Vec<u8>>(path) {
		Ok(data) => data,
		Err(_) => return,
	};

	output.stats.found += 1;
	let sha256 = hex_sha256(&data);
	output.mappings.push(MappingEntry {
		path: path.to_owned(),
		sha256: sha256.clone(),
	});

	match process_texture(&data, sha256, deduper) {
		Ok(ProcessResult::Archived(entry)) => {
			output.stats.converted += 1;
			output.stats.archived += 1;
			output.archive_entries.push(entry);
		}
		Ok(ProcessResult::Duplicate) => {
			output.stats.duplicates += 1;
		}
		Err(error) => {
			output.stats.errors += 1;
			output.errors.push(format!("{path}: {error:#}"));
		}
	}
}

fn process_texture(data: &[u8], sha256: String, deduper: &Deduper) -> Result<ProcessResult> {
	{
		let mut hashes = deduper.lock().expect("deduper mutex poisoned");
		if !hashes.insert(sha256.clone()) {
			return Ok(ProcessResult::Duplicate);
		}
	}

	let webp = convert_tex_to_webp(data)?;
	Ok(ProcessResult::Archived(ArchiveEntry { sha256, webp }))
}

enum ProcessResult {
	Archived(ArchiveEntry),
	Duplicate,
}

fn hex_sha256(data: &[u8]) -> String {
	let digest = Sha256::digest(data);
	let mut out = String::with_capacity(digest.len() * 2);
	for byte in digest {
		use std::fmt::Write as _;
		let _ = write!(&mut out, "{byte:02x}");
	}
	out
}
