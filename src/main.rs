mod config;
mod export;
mod tex;

use std::{
	fs::{self, File},
	io::BufWriter,
	path::Path,
	sync::{
		Arc,
		atomic::{AtomicU32, Ordering},
		mpsc,
	},
	thread,
	time::{Duration, Instant},
};

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use ironworks::{
	Ironworks,
	sqpack::{Install, SqPack},
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::{
	config::{
		END_ID_EXCLUSIVE, OutputMode, START_ID, output_root, parse_options, resolve_install_root,
	},
	export::{ExportProcessor, OutputWriter, new_deduper},
};

fn main() -> Result<()> {
	let started_at = Instant::now();
	let options = parse_options()?;
	let install_root = resolve_install_root(options.install_root)
		.context("could not determine FFXIV install path from the provided path, current working directory, or known global install paths")?;

	let ui_output_dir = output_root();
	fs::create_dir_all(&ui_output_dir)
		.with_context(|| format!("create output dir {}", ui_output_dir.display()))?;

	let archive_path = ui_output_dir.join(options.output_format.archive_name());
	let mapping_path = ui_output_dir.join("icon-path-sha256.json");

	let total = u64::from(END_ID_EXCLUSIVE - START_ID);
	let progress = ProgressBar::new(total);
	progress.set_style(
		ProgressStyle::with_template(
			"ui icons [{bar:40.cyan/blue}] {pos}/{len} {percent}% {msg}",
		)?
		.progress_chars("=>-"),
	);

	let mut writer = match options.output_mode {
		OutputMode::Archive => {
			if archive_path.exists() {
				fs::remove_file(&archive_path).with_context(|| {
					format!("remove existing archive {}", archive_path.display())
				})?;
			}

			let archive_file = File::create(&archive_path)
				.with_context(|| format!("create archive {}", archive_path.display()))?;
			let zip = ZipWriter::new(BufWriter::new(archive_file));
			let zip_options =
				SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
			OutputWriter::archive(zip, zip_options, options.output_format)
		}
		OutputMode::Files => OutputWriter::files(ui_output_dir.clone(), options.output_format),
	};
	let worker_count = thread::available_parallelism()
		.map(|parallelism| parallelism.get())
		.unwrap_or(1);
	let next_id = Arc::new(AtomicU32::new(START_ID));
	let deduper = new_deduper();
	let (sender, receiver) = mpsc::channel();
	let mut workers = Vec::with_capacity(worker_count);

	for _ in 0..worker_count {
		let install_root = install_root.clone();
		let next_id = Arc::clone(&next_id);
		let deduper = Arc::clone(&deduper);
		let output_format = options.output_format;
		let output_mode = options.output_mode;
		let output_root = match output_mode {
			OutputMode::Archive => None,
			OutputMode::Files => Some(ui_output_dir.clone()),
		};
		let sender = sender.clone();

		workers.push(thread::spawn(move || {
			let ironworks = create_ironworks(&install_root);
			let processor =
				ExportProcessor::new(&ironworks, &deduper, output_format, output_mode, output_root);

			loop {
				let id = next_id.fetch_add(1, Ordering::Relaxed);
				if id >= END_ID_EXCLUSIVE {
					break;
				}

				if sender
					.send(processor.process_id(id))
					.is_err()
				{
					break;
				}
			}
		}));
	}
	drop(sender);

	for _ in START_ID..END_ID_EXCLUSIVE {
		let output = receiver
			.recv()
			.context("worker channel closed before all ids were processed")?;
		writer.write_output(output)?;
		let stats = writer.stats();

		progress.inc(1);
		progress.set_message(format!(
			"found:{} converted:{} archived:{} dup:{} exists:{} err:{}",
			stats.found,
			stats.converted,
			stats.archived,
			stats.duplicates,
			stats.skipped_existing,
			stats.errors,
		));
	}

	for worker in workers {
		worker.join().map_err(|panic| {
			anyhow::anyhow!("worker thread panicked: {}", panic_message(&panic))
		})?;
	}

	let (mappings, stats) = writer.finish()?;

	progress.finish_with_message(format!(
		"found:{} converted:{} archived:{} dup:{} exists:{} err:{}",
		stats.found,
		stats.converted,
		stats.archived,
		stats.duplicates,
		stats.skipped_existing,
		stats.errors,
	));

	let mapping_json =
		serde_json::to_vec_pretty(&mappings).context("serialize mapping file")?;
	fs::write(&mapping_path, [&mapping_json[..], b"\n"].concat())
		.with_context(|| format!("write mapping {}", mapping_path.display()))?;

	match options.output_mode {
		OutputMode::Archive => println!("archive: {}", archive_path.display()),
		OutputMode::Files => println!("files: {}", ui_output_dir.display()),
	}
	println!("mapping: {}", mapping_path.display());
	println!("elapsed: {}", format_elapsed(started_at.elapsed()));

	Ok(())
}

fn create_ironworks(install_root: &Path) -> Ironworks {
	let install = Install::at(install_root);
	let sqpack = SqPack::new(install);
	let mut ironworks = Ironworks::new();
	ironworks.add_resource(sqpack);
	ironworks
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> &str {
	if let Some(message) = payload.downcast_ref::<&'static str>() {
		message
	} else if let Some(message) = payload.downcast_ref::<String>() {
		message.as_str()
	} else {
		"unknown panic payload"
	}
}

fn format_elapsed(duration: Duration) -> String {
	let total_seconds = duration.as_secs();
	let hours = total_seconds / 3600;
	let minutes = (total_seconds % 3600) / 60;
	let seconds = total_seconds % 60;
	let milliseconds = duration.subsec_millis();

	if hours > 0 {
		format!("{hours}h {minutes}m {seconds}.{milliseconds:03}s")
	} else if minutes > 0 {
		format!("{minutes}m {seconds}.{milliseconds:03}s")
	} else {
		format!("{seconds}.{milliseconds:03}s")
	}
}
