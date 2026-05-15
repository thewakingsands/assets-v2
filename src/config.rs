use std::{env, path::PathBuf};

pub const VERSIONS: [&str; 7] = ["", "/en", "/ja", "/fr", "/de", "/hq", "/chs"];
pub const START_ID: u32 = 0;
pub const END_ID_EXCLUSIVE: u32 = 1_000_000;
pub const WEBP_QUALITY: f32 = 50.0;

const TRY_INSTALL_PATHS: &[&str] = &[
	r"C:\Games\FINAL FANTASY XIV",
	r"C:\SquareEnix\FINAL FANTASY XIV - A Realm Reborn",
	r"C:\Program Files (x86)\Steam\steamapps\common\FINAL FANTASY XIV Online",
	r"C:\Program Files (x86)\Steam\steamapps\common\FINAL FANTASY XIV - A Realm Reborn",
	r"C:\Program Files (x86)\FINAL FANTASY XIV - A Realm Reborn",
	r"C:\Program Files (x86)\SquareEnix\FINAL FANTASY XIV - A Realm Reborn",
];

const REQUIRED_INDEX_PATH: &str = r"game\sqpack\ffxiv\060000.win32.index";

pub fn resolve_install_root() -> Option<PathBuf> {
	if let Some(path) = env::args_os().nth(1).map(PathBuf::from)
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
