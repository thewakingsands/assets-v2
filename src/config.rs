use std::{env, path::PathBuf};

pub const VERSIONS: [&str; 7] = ["", "/en", "/ja", "/fr", "/de", "/hq", "/chs"];
pub const START_ID: u32 = 0;
pub const END_ID_EXCLUSIVE: u32 = 1_000_000;
pub const WEBP_QUALITY: f32 = 50.0;
pub const AVIF_QUALITY: f32 = 50.0;

const TRY_INSTALL_PATHS: &[&str] = &[
	r"C:\Games\FINAL FANTASY XIV",
	r"C:\SquareEnix\FINAL FANTASY XIV - A Realm Reborn",
	r"C:\Program Files (x86)\Steam\steamapps\common\FINAL FANTASY XIV Online",
	r"C:\Program Files (x86)\Steam\steamapps\common\FINAL FANTASY XIV - A Realm Reborn",
	r"C:\Program Files (x86)\FINAL FANTASY XIV - A Realm Reborn",
	r"C:\Program Files (x86)\SquareEnix\FINAL FANTASY XIV - A Realm Reborn",
];

const REQUIRED_INDEX_PATH: &str = r"game\sqpack\ffxiv\060000.win32.index";

#[derive(Debug, Clone, Copy)]
pub enum OutputFormat {
	Webp,
	Avif,
}

impl OutputFormat {
	pub fn extension(self) -> &'static str {
		match self {
			Self::Webp => "webp",
			Self::Avif => "avif",
		}
	}

	pub fn archive_name(self) -> String {
		format!("icons.{}.zip", self.extension())
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
	Archive,
	Files,
}

#[derive(Debug, Clone)]
pub struct AppOptions {
	pub install_root: Option<PathBuf>,
	pub output_format: OutputFormat,
	pub output_mode: OutputMode,
}

pub fn parse_options() -> anyhow::Result<AppOptions> {
	let mut install_root = None;
	let mut output_format = OutputFormat::Webp;
	let mut output_mode = OutputMode::Archive;
	let mut args = env::args_os().skip(1);

	while let Some(arg) = args.next() {
		match arg.to_str() {
			Some("--format") => {
				let Some(value) = args.next() else {
					anyhow::bail!("missing value for --format");
				};

				output_format = match value.to_string_lossy().to_ascii_lowercase().as_str() {
					"webp" => OutputFormat::Webp,
					"avif" => OutputFormat::Avif,
					other => anyhow::bail!(
						"unsupported output format `{other}`; expected `webp` or `avif`"
					),
				};
			}
			Some(flag) if flag.starts_with("--format=") => {
				let value = &flag["--format=".len()..];
				output_format = match value {
					"webp" => OutputFormat::Webp,
					"avif" => OutputFormat::Avif,
					other => anyhow::bail!(
						"unsupported output format `{other}`; expected `webp` or `avif`"
					),
				};
			}
			Some("--output-mode") => {
				let Some(value) = args.next() else {
					anyhow::bail!("missing value for --output-mode");
				};

				output_mode = match value.to_string_lossy().to_ascii_lowercase().as_str() {
					"archive" => OutputMode::Archive,
					"files" => OutputMode::Files,
					other => anyhow::bail!(
						"unsupported output mode `{other}`; expected `archive` or `files`"
					),
				};
			}
			Some(flag) if flag.starts_with("--output-mode=") => {
				let value = &flag["--output-mode=".len()..];
				output_mode = match value {
					"archive" => OutputMode::Archive,
					"files" => OutputMode::Files,
					other => anyhow::bail!(
						"unsupported output mode `{other}`; expected `archive` or `files`"
					),
				};
			}
			Some("--no-archive") => {
				output_mode = OutputMode::Files;
			}
			Some(other) if other.starts_with("--") => {
				anyhow::bail!("unsupported option `{other}`");
			}
			_ => {
				if install_root.is_some() {
					anyhow::bail!("multiple install-root paths provided");
				}
				install_root = Some(PathBuf::from(arg));
			}
		}
	}

	Ok(AppOptions {
		install_root,
		output_format,
		output_mode,
	})
}

pub fn resolve_install_root(cli_install_root: Option<PathBuf>) -> Option<PathBuf> {
	if let Some(path) = cli_install_root
		&& is_valid_install_root(&path)
	{
		return Some(path);
	}

	if let Ok(path) = env::current_dir()
		&& is_valid_install_root(&path)
	{
		return Some(path);
	}

	TRY_INSTALL_PATHS
		.iter()
		.map(PathBuf::from)
		.find(|path| is_valid_install_root(path))
}

pub fn output_root() -> PathBuf {
	env::var_os("ASSETS_OUTPUT_DIR")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from("outputs"))
}

fn is_valid_install_root(path: &std::path::Path) -> bool {
	path.join(REQUIRED_INDEX_PATH).is_file()
}
