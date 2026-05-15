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
};

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use ironworks::{
	Ironworks,
	sqpack::{Install, SqPack},
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::{
	config::{END_ID_EXCLUSIVE, START_ID, output_root, resolve_install_root},
	export::{ArchiveWriter, new_deduper, process_id},
};

fn main() -> Result<()> {
	let install_root = resolve_install_root()
		.context("could not determine FFXIV install path from argv[1], current working directory, or known global install paths")?;

	let ui_output_dir = output_root().join("ui");
	fs::create_dir_all(&ui_output_dir)
		.with_context(|| format!("create output dir {}", ui_output_dir.display()))?;

	let archive_path = ui_output_dir.join("icons.zip");
	let mapping_path = ui_output_dir.join("icon-path-sha256.json");

	if archive_path.exists() {
		fs::remove_file(&archive_path)
			.with_context(|| format!("remove existing archive {}", archive_path.display()))?;
	}

	let archive_file = File::create(&archive_path)
		.with_context(|| format!("create archive {}", archive_path.display()))?;
	let writer = BufWriter::new(archive_file);
	let zip = ZipWriter::new(writer);
	let zip_options =
		SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

	let total = u64::from(END_ID_EXCLUSIVE - START_ID);
	let progress = ProgressBar::new(total);
	progress.set_style(
		ProgressStyle::with_template(
			"ui icons [{bar:40.cyan/blue}] {pos}/{len} {percent}% {msg}",
		)?
		.progress_chars("=>-"),
	);

	let mut archive = ArchiveWriter::new(zip, zip_options);
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
		let sender = sender.clone();

		workers.push(thread::spawn(move || {
			let ironworks = create_ironworks(&install_root);

			loop {
				let id = next_id.fetch_add(1, Ordering::Relaxed);
				if id >= END_ID_EXCLUSIVE {
					break;
				}

				if sender.send(process_id(&ironworks, &deduper, id)).is_err() {
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
		archive.write_output(output)?;
		let stats = archive.stats();

		progress.inc(1);
		progress.set_message(format!(
			"found:{} converted:{} archived:{} dup:{} err:{}",
			stats.found, stats.converted, stats.archived, stats.duplicates, stats.errors,
		));
	}

	for worker in workers {
		worker.join().map_err(|panic| {
			anyhow::anyhow!("worker thread panicked: {}", panic_message(&panic))
		})?;
	}

	let (_writer, mappings, stats) = archive.finish()?;

	progress.finish_with_message(format!(
		"found:{} converted:{} archived:{} dup:{} err:{}",
		stats.found, stats.converted, stats.archived, stats.duplicates, stats.errors,
	));

	let mapping_json =
		serde_json::to_vec_pretty(&mappings).context("serialize mapping file")?;
	fs::write(&mapping_path, [&mapping_json[..], b"\n"].concat())
		.with_context(|| format!("write mapping {}", mapping_path.display()))?;

	println!("archive: {}", archive_path.display());
	println!("mapping: {}", mapping_path.display());

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
