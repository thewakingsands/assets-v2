use std::{
	collections::HashSet,
	fs::{self, File},
	io::{BufWriter, Write},
	path::{Path, PathBuf},
	sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use ironworks::Ironworks;
use serde::Serialize;
use sha2::{Digest, Sha256};
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::{
	config::{OutputFormat, OutputMode, VERSIONS},
	tex::convert_tex,
};

#[derive(Debug, Serialize)]
pub struct MappingEntry {
	id: u32,
	version: String,
	hr: bool,
	sha256: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ExportStats {
	pub found: usize,
	pub converted: usize,
	pub archived: usize,
	pub duplicates: usize,
	pub skipped_existing: usize,
	pub errors: usize,
}

impl ExportStats {
	pub fn merge(&mut self, other: ExportStats) {
		self.found += other.found;
		self.converted += other.converted;
		self.archived += other.archived;
		self.duplicates += other.duplicates;
		self.skipped_existing += other.skipped_existing;
		self.errors += other.errors;
	}
}

pub struct ArchiveEntry {
	sha256: String,
	data: Vec<u8>,
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

pub enum OutputWriter {
	Archive(ArchiveWriter),
	Files(FileWriter),
}

impl OutputWriter {
	pub fn archive(
		zip: ZipWriter<BufWriter<File>>,
		zip_options: SimpleFileOptions,
		output_format: OutputFormat,
	) -> Self {
		Self::Archive(ArchiveWriter::new(zip, zip_options, output_format))
	}

	pub fn files(output_root: PathBuf, output_format: OutputFormat) -> Self {
		Self::Files(FileWriter::new(output_root, output_format))
	}

	pub fn write_output(&mut self, output: IdOutput) -> Result<()> {
		match self {
			Self::Archive(writer) => writer.write_output(output),
			Self::Files(writer) => writer.write_output(output),
		}
	}

	pub fn stats(&self) -> ExportStats {
		match self {
			Self::Archive(writer) => writer.stats(),
			Self::Files(writer) => writer.stats(),
		}
	}

	pub fn finish(self) -> Result<(Vec<MappingEntry>, ExportStats)> {
		match self {
			Self::Archive(writer) => writer.finish(),
			Self::Files(writer) => writer.finish(),
		}
	}
}

struct ArchiveWriter {
	zip: ZipWriter<BufWriter<File>>,
	zip_options: SimpleFileOptions,
	output_format: OutputFormat,
	mappings: Vec<MappingEntry>,
	stats: ExportStats,
}

impl ArchiveWriter {
	fn new(
		zip: ZipWriter<BufWriter<File>>,
		zip_options: SimpleFileOptions,
		output_format: OutputFormat,
	) -> Self {
		Self {
			zip,
			zip_options,
			output_format,
			mappings: Vec::new(),
			stats: ExportStats::default(),
		}
	}

	fn write_output(&mut self, output: IdOutput) -> Result<()> {
		for entry in output.archive_entries {
			let relative_path = encoded_relative_path(&entry.sha256, self.output_format);
			let zip_path = relative_path.to_string_lossy().replace('\\', "/");
			self.zip
				.start_file(zip_path, self.zip_options)
				.with_context(|| format!("start zip entry for {}", entry.sha256))?;
			self.zip
				.write_all(&entry.data)
				.with_context(|| format!("write zip entry for {}", entry.sha256))?;
			self.stats.archived += 1;
		}

		self.mappings.extend(output.mappings);
		self.stats.merge(output.stats);

		for error in output.errors {
			eprintln!("{error}");
		}

		Ok(())
	}

	fn stats(&self) -> ExportStats {
		self.stats
	}

	fn finish(self) -> Result<(Vec<MappingEntry>, ExportStats)> {
		let mut writer = self.zip.finish().context("finalize zip archive")?;
		writer.flush().context("flush archive output")?;
		Ok((self.mappings, self.stats))
	}
}

struct FileWriter {
	output_root: PathBuf,
	output_format: OutputFormat,
	mappings: Vec<MappingEntry>,
	stats: ExportStats,
}

impl FileWriter {
	fn new(output_root: PathBuf, output_format: OutputFormat) -> Self {
		Self {
			output_root,
			output_format,
			mappings: Vec::new(),
			stats: ExportStats::default(),
		}
	}

	fn write_output(&mut self, output: IdOutput) -> Result<()> {
		for entry in output.archive_entries {
			let file_path = encoded_full_path(&self.output_root, &entry.sha256, self.output_format);
			if file_path.exists() {
				self.stats.skipped_existing += 1;
				continue;
			}

			if let Some(parent) = file_path.parent() {
				fs::create_dir_all(parent)
					.with_context(|| format!("create output dir {}", parent.display()))?;
			}

			fs::write(&file_path, &entry.data)
				.with_context(|| format!("write encoded file {}", file_path.display()))?;
			self.stats.archived += 1;
		}

		self.mappings.extend(output.mappings);
		self.stats.merge(output.stats);

		for error in output.errors {
			eprintln!("{error}");
		}

		Ok(())
	}

	fn stats(&self) -> ExportStats {
		self.stats
	}

	fn finish(self) -> Result<(Vec<MappingEntry>, ExportStats)> {
		Ok((self.mappings, self.stats))
	}
}

pub struct ExportProcessor<'a> {
	ironworks: &'a Ironworks,
	deduper: &'a Deduper,
	output_format: OutputFormat,
	output_mode: OutputMode,
	output_root: Option<PathBuf>,
}

impl<'a> ExportProcessor<'a> {
	pub fn new(
		ironworks: &'a Ironworks,
		deduper: &'a Deduper,
		output_format: OutputFormat,
		output_mode: OutputMode,
		output_root: Option<PathBuf>,
	) -> Self {
		Self {
			ironworks,
			deduper,
			output_format,
			output_mode,
			output_root,
		}
	}

	pub fn process_id(&self, id: u32) -> IdOutput {
		let mut output = IdOutput {
			mappings: Vec::new(),
			archive_entries: Vec::new(),
			stats: ExportStats::default(),
			errors: Vec::new(),
		};

		for version in VERSIONS {
			let has_hr = self.process_path(id, version, true, &mut output);

			if !has_hr {
				self.process_path(id, version, false, &mut output);
			}
		}

		output
	}

	fn process_path(&self, id: u32, version: &str, hr: bool, output: &mut IdOutput) -> bool {
		let suffix = match hr {
			true => "_hr1.tex",
			false => ".tex",
		};
		let id_string = format!("{id:06}");
		let path = format!("ui/icon/{}000{version}/{id_string}{suffix}", &id_string[..3]);
		let data = match self.ironworks.file::<Vec<u8>>(&path) {
			Ok(data) => data,
			Err(_) => return false,
		};

		output.stats.found += 1;
		let sha256 = hex_sha256(&data);
		output.mappings.push(MappingEntry {
			id,
			version: version.to_owned(),
			hr,
			sha256: sha256.clone(),
		});

		match self.process_texture(&data, &sha256) {
			Ok(ProcessResult::Archived(entry)) => {
				output.stats.converted += 1;
				output.archive_entries.push(entry);
				true
			}
			Ok(ProcessResult::Duplicate) => {
				output.stats.duplicates += 1;
				true
			}
			Ok(ProcessResult::ExistingFile) => {
				output.stats.skipped_existing += 1;
				true
			}
			Err(error) => {
				output.stats.errors += 1;
				output.errors.push(format!("{path}: {error:#}"));
				false
			}
		}
	}

	fn process_texture(&self, data: &[u8], sha256: &str) -> Result<ProcessResult> {
		{
			let mut hashes = self.deduper.lock().expect("deduper mutex poisoned");
			if !hashes.insert(sha256.to_owned()) {
				return Ok(ProcessResult::Duplicate);
			}
		}

		if self.output_mode == OutputMode::Files
			&& let Some(root) = &self.output_root
		{
			let file_path = encoded_full_path(root, sha256, self.output_format);
			if file_path.exists() {
				return Ok(ProcessResult::ExistingFile);
			}
		}

		let encoded = convert_tex(data, self.output_format)?;
		Ok(ProcessResult::Archived(ArchiveEntry {
			sha256: sha256.to_owned(),
			data: encoded,
		}))
	}
}

enum ProcessResult {
	Archived(ArchiveEntry),
	Duplicate,
	ExistingFile,
}

pub fn encoded_relative_path(sha256: &str, output_format: OutputFormat) -> PathBuf {
	let shard = &sha256[..sha256.len().min(2)];
	PathBuf::from(shard).join(format!("{sha256}.{}", output_format.extension()))
}

pub fn encoded_full_path(
	output_root: &Path,
	sha256: &str,
	output_format: OutputFormat,
) -> PathBuf {
	output_root.join(encoded_relative_path(sha256, output_format))
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
