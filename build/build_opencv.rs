use std::env;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};

use crate::header::IncludePath;
use crate::library::{EnvList, Library};
use crate::{Result, OUT_DIR};
use flate2::read::GzDecoder;
use glob::glob;
use opencv_binding_generator::IteratorExt;
use reqwest::blocking::Client;
use semver::Version;
use tar::Archive;

const DEFAULT_OPENCV_VERSION: &str = "4.9.0";

pub fn build_from_source() -> Result<Library> {
	let version = env::var("OPENCV_SOURCE_VERSION").unwrap_or_else(|_| DEFAULT_OPENCV_VERSION.to_string());
	eprintln!("=== Building OpenCV version {} from source", version);

	let build_dir = OUT_DIR.join("opencv_build");
	fs::create_dir_all(&build_dir)?;
	let install_dir = OUT_DIR.join("opencv_install");
	let source_dir = download_and_extract_tar(
		&format!("https://github.com/opencv/opencv/archive/{}.tar.gz", version),
		&build_dir,
		&format!("opencv-{}", version),
	)?;

	let contrib_dir = if env::var("CARGO_FEATURE_CONTRIB").is_ok() {
		Some(download_and_extract_tar(
			&format!("https://github.com/opencv/opencv_contrib/archive/{}.tar.gz", version),
			&build_dir,
			&format!("opencv_contrib-{}", version),
		)?)
	} else {
		None
	};

	build_opencv(&source_dir, &build_dir, &install_dir, contrib_dir.as_deref())?;

	let include_path = install_dir.join("include").join("opencv4");
	let lib_path = install_dir.join("lib");
	let link_libs = {
		let pattern = if cfg!(target_os = "macos") {
			lib_path.join("libopencv_*.dylib").to_string_lossy().to_string()
		} else {
			lib_path.join("libopencv_*.so").to_string_lossy().to_string()
		};
		glob(&pattern)?
			.into_iter()
			.filter_map(|e| e.ok())
			.map(|p| p.to_string_lossy().to_string())
			.join(",")
	};

	let include_path_str = include_path.display().to_string();
	let lib_path_str = lib_path.display().to_string();
	Library::probe_from_paths(
		Some(EnvList::from(include_path_str.as_str())),
		Some(EnvList::from(lib_path_str.as_str())),
		Some(EnvList::from(link_libs.as_str())),
	)
}

fn download_and_extract_tar(url: &str, build_dir: &Path, expected_dir_name: &str) -> Result<PathBuf> {
	eprintln!("=== Downloading archive from {}", url);
	let response = reqwest::blocking::get(url)?.error_for_status()?;
	let archive_content = response.bytes()?;
	eprintln!("=== Extracting archive");
	let mut cursor = Cursor::new(archive_content);
	let tar = GzDecoder::new(&mut cursor);
	let mut archive = Archive::new(tar);
	archive.unpack(build_dir)?;
	let target_dir = build_dir.join(expected_dir_name);
	if !target_dir.exists() {
		return Err(format!("Failed to extract archive: {}", target_dir.display()).into());
	}
	eprintln!("=== Extraction completed");
	Ok(target_dir)
}

fn build_opencv(source_dir: &Path, build_dir: &Path, install_dir: &Path, contrib_dir: Option<&Path>) -> Result<()> {
	eprintln!("=== Configuring and building OpenCV with CMake");
	let cmake_build_dir = build_dir.join("build");
	fs::create_dir_all(&cmake_build_dir)?;

	let mut config = cmake::Config::new(source_dir);
	config
		.out_dir(&cmake_build_dir)
		.define("CMAKE_INSTALL_PREFIX", install_dir.to_str().unwrap())
		.define("CMAKE_BUILD_TYPE", "Release")
		.define("BUILD_SHARED_LIBS", "ON")
		.define("BUILD_TESTS", "OFF")
		.define("BUILD_PERF_TESTS", "OFF")
		.define("BUILD_EXAMPLES", "OFF")
		.define("BUILD_DOCS", "OFF")
		.define(
			"CMAKE_BUILD_PARALLEL_LEVEL",
			std::thread::available_parallelism().map(|p| p.get()).unwrap_or(2).to_string(),
		)
		.define("BUILD_opencv_java", "OFF")
		.define("BUILD_opencv_python", "OFF")
		.define("BUILD_opencv_python2", "OFF")
		.define("BUILD_opencv_python3", "OFF");
	if cfg!(target_os = "macos") {
		config
			.define("CMAKE_OSX_DEPLOYMENT_TARGET", "10.13")
			.define("CMAKE_CXX_FLAGS", "-stdlib=libc++");
	}

	if let Some(contrib_path) = contrib_dir {
		eprintln!("=== Including OpenCV contrib modules from {}", contrib_path.display());
		config.define("OPENCV_EXTRA_MODULES_PATH", contrib_path.join("modules").to_str().unwrap());
	}

	let _dst = config.build();

	eprintln!(
		"=== OpenCV build completed successfully, install directory: {}",
		install_dir.display()
	);
	Ok(())
}
