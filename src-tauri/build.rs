use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let runtimes_dir = manifest_dir.join("runtimes");

    println!("cargo:warning==== build.rs diagnostics ====");
    println!("cargo:warning=CARGO_MANIFEST_DIR: {}", manifest_dir.display());
    println!("cargo:warning=runtimes dir exists: {}", runtimes_dir.exists());

    if runtimes_dir.exists() {
        // List top-level contents
        if let Ok(entries) = std::fs::read_dir(&runtimes_dir) {
            for e in entries.flatten() {
                let ft = if e.path().is_dir() { "DIR" } else { "FILE" };
                println!("cargo:warning=  [{}] {}", ft, e.file_name().to_string_lossy());
            }
        }

        // Walk uv/ directory
        let uv_dir = runtimes_dir.join("uv");
        println!("cargo:warning=uv dir exists: {}", uv_dir.exists());
        if uv_dir.exists() {
            walk_and_log(&uv_dir, "uv");
        }

        // Walk node/ directory (just count)
        let node_dir = runtimes_dir.join("node");
        println!("cargo:warning=node dir exists: {}", node_dir.exists());
        if node_dir.exists() {
            let count = count_files(&node_dir);
            println!("cargo:warning=node file count: {}", count);
        }

        // Direct test: can we read python.exe?
        let py_exe = runtimes_dir.join("uv").join("python").join("python.exe");
        println!("cargo:warning=python.exe path: {}", py_exe.display());
        println!("cargo:warning=python.exe exists: {}", py_exe.exists());
        if py_exe.exists() {
            let meta = std::fs::metadata(&py_exe);
            println!("cargo:warning=python.exe metadata: {:?}", meta);
        }

        // Check if python dir is a symlink/junction
        let py_dir = runtimes_dir.join("uv").join("python");
        if py_dir.exists() {
            let meta = std::fs::symlink_metadata(&py_dir);
            println!("cargo:warning=python dir symlink_metadata: {:?}", meta);
            let is_symlink = std::fs::symlink_metadata(&py_dir)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            println!("cargo:warning=python dir is_symlink: {}", is_symlink);

            // Try reading directory contents
            if let Ok(entries) = std::fs::read_dir(&py_dir) {
                let mut count = 0;
                for e in entries.flatten() {
                    count += 1;
                    if count <= 10 {
                        let ft = if e.path().is_dir() { "DIR" } else { "FILE" };
                        println!("cargo:warning=  python/[{}] {}", ft, e.file_name().to_string_lossy());
                    }
                }
                println!("cargo:warning=python dir entry count: {}", count);
            } else {
                println!("cargo:warning=FAILED to read python dir!");
            }
        }
    }

    // Now run the actual Tauri build
    println!("cargo:warning==== end build.rs diagnostics ====");
    tauri_build::build()
}

fn walk_and_log(dir: &Path, prefix: &str) {
    let mut file_count = 0usize;
    let mut dir_count = 0usize;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                dir_count += 1;
                let (fc, dc) = count_all(&path);
                file_count += fc;
                dir_count += dc;
                if dir_count <= 5 {
                    println!(
                        "cargo:warning={}/{}: DIR ({} files, {} dirs)",
                        prefix,
                        e.file_name().to_string_lossy(),
                        fc,
                        dc
                    );
                }
            } else {
                file_count += 1;
                if file_count <= 10 {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    println!(
                        "cargo:warning={}/{}: FILE ({} bytes)",
                        prefix,
                        e.file_name().to_string_lossy(),
                        size
                    );
                }
            }
        }
    }
    println!(
        "cargo:warning={}: total {} files, {} dirs",
        prefix, file_count, dir_count
    );
}

fn count_all(dir: &Path) -> (usize, usize) {
    let mut file_count = 0usize;
    let mut dir_count = 0usize;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                dir_count += 1;
                let (fc, dc) = count_all(&path);
                file_count += fc;
                dir_count += dc;
            } else {
                file_count += 1;
            }
        }
    }
    (file_count, dir_count)
}

fn count_files(dir: &Path) -> usize {
    let mut count = 0usize;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                count += count_files(&path);
            } else {
                count += 1;
            }
        }
    }
    count
}
