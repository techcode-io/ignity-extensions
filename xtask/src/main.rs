use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let args: Vec<String> = env::args().collect();
    let task = args.get(1).map(|s| s.as_str()).unwrap_or("build");

    match task {
        "build" => build_packages(),
        _ => eprintln!("Unknown task: {}", task),
    }
}

fn build_packages() {
    let dist_dir = PathBuf::from("dist");

    // Create dist directory
    fs::create_dir_all(&dist_dir).expect("Failed to create dist directory");

    let version = env::var("VERSION").unwrap_or_else(|_| {
        // Get version from Cargo.toml or git
        "0.1.0".to_string()
    });

    println!("Building packages with version: {}", version);

    // Build .deb package
    run_nfpm("deb", &version);

    // Build .rpm package
    run_nfpm("rpm", &version);

    println!("Packages built successfully in {:?}", dist_dir);
}

fn run_nfpm(packager: &str, version: &str) {
    let output = Command::new("nfpm")
        .arg("package")
        .arg("--config")
        .arg("nfpm.yaml")
        .arg("--packager")
        .arg(packager)
        .arg("--target")
        .arg("dist")
        .env("VERSION", version)
        .output()
        .unwrap_or_else(|_| panic!("Failed to run nfpm for {}", packager));

    if !output.status.success() {
        eprintln!(
            "nfpm build failed for {}:\n{}",
            packager,
            String::from_utf8_lossy(&output.stderr)
        );
        std::process::exit(1);
    }

    println!("Built {} package", packager);
}
