use std::env;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use glob::glob;
use reqwest::blocking::Client;
use semver::Version;
use tar::Archive;

use crate::header::IncludePath;
use crate::library::{EnvList, Library};
use crate::{Result, OUT_DIR};

/// Default OpenCV version to build if not specified
const DEFAULT_OPENCV_VERSION: &str = "4.9.0";

/// Build OpenCV from source
pub fn build_from_source() -> Result<Library> {
	// Determine which version to build
	let version = env::var("OPENCV_SOURCE_VERSION").unwrap_or_else(|_| DEFAULT_OPENCV_VERSION.to_string());
	eprintln!("=== Building OpenCV version {} from source", version);

	// Create build directories
	let build_dir = OUT_DIR.join("opencv_build");
	let install_dir = OUT_DIR.join("opencv_install");

	// Create the build directory if it doesn't exist
	fs::create_dir_all(&build_dir)?;

	// Define source paths
	let with_contrib = env::var("CARGO_FEATURE_CONTRIB").is_ok();

	let source_dir = download_and_extract_tar(
		&format!("https://github.com/opencv/opencv/archive/{}.tar.gz", version),
		&build_dir,
		&format!("opencv-{}", version),
	)?;

	let contrib_dir = if with_contrib {
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

		let mut libs = Vec::new();
		for entry in glob(&pattern)? {
			if let Ok(path) = entry {
				libs.push(path.to_string_lossy().to_string());
			}
		}

		// If no libraries found, fall back to default set
		if libs.is_empty() {
			return Err(format!("=== No OpenCV libraries found in {}, using default list", lib_path.display()).into());
		}
		eprintln!("=== Found {} OpenCV libraries in {}", libs.len(), lib_path.display());
		libs.join(",")
	};

	let include_path_str = include_path.display().to_string();
	let lib_path_str = lib_path.display().to_string();
	Library::probe_from_paths(
		Some(EnvList::from(include_path_str.as_str())),
		Some(EnvList::from(lib_path_str.as_str())),
		Some(EnvList::from(link_libs.as_str())),
	)
}

/// Download and extract a tar.gz file using reqwest, flate2 and tar crates
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

/// Build OpenCV using CMake
fn build_opencv(source_dir: &Path, build_dir: &Path, install_dir: &Path, contrib_dir: Option<&Path>) -> Result<()> {
	eprintln!("=== Configuring and building OpenCV with CMake");

	// Create cmake build directory
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
        .define("CMAKE_BUILD_PARALLEL_LEVEL", std::thread::available_parallelism().map(|p| p.get()).unwrap_or(2).to_string())
        // Disable modules we don't need
        .define("BUILD_opencv_java", "OFF")
        .define("BUILD_opencv_python", "OFF")
        .define("BUILD_opencv_python2", "OFF")
        .define("BUILD_opencv_python3", "OFF");

	// Platform-specific configurations
	if cfg!(target_os = "macos") {
		// macOS-specific options
		config.define("CMAKE_OSX_DEPLOYMENT_TARGET", "10.13");

		// Use libc++ on macOS
		config.define("CMAKE_CXX_FLAGS", "-stdlib=libc++");
	}

	// Add contrib modules path if requested
	if let Some(contrib_path) = contrib_dir {
		eprintln!("=== Including OpenCV contrib modules from {}", contrib_path.display());
		config.define("OPENCV_EXTRA_MODULES_PATH", contrib_path.join("modules").to_str().unwrap());
	}

	// Add additional CMake options from environment if available
	if let Ok(extra_opts) = env::var("OPENCV_CMAKE_OPTIONS") {
		for opt in extra_opts.split(';') {
			if !opt.trim().is_empty() {
				let parts: Vec<&str> = opt.trim().splitn(2, '=').collect();
				if parts.len() == 2 {
					// Remove leading -D if present
					let option_name = parts[0].trim_start_matches("-D");
					config.define(option_name, parts[1]);
				}
			}
		}
	}

	// Configure, build and install
	let dst = config.build();

	eprintln!("=== OpenCV build completed successfully");
	eprintln!("=== Install directory: {}", install_dir.display());

	Ok(())
}
