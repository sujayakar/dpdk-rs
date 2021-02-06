use bindgen::Builder;
use std::process::Command;
use std::path::Path;
use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=DPDK_PATH");

    let out_dir_s = env::var("OUT_DIR").unwrap();
    let out_dir = Path::new(&out_dir_s);
    let dpdk_path_s = env::var("DPDK_PATH").unwrap();
    let dpdk_path = Path::new(&dpdk_path_s);
    let pkg_config_path = dpdk_path.join("lib/x86_64-linux-gnu/pkgconfig");
    let cflags_bytes = Command::new("pkg-config")
        .env("PKG_CONFIG_PATH", &pkg_config_path)
        .args(&["--cflags", "libdpdk"])
        .output()
        .unwrap_or_else(|e| panic!("Failed pkg-config cflags: {:?}", e))
        .stdout;
    let cflags = String::from_utf8(cflags_bytes).unwrap();

    let mut header_locations = vec![];
    for flag in cflags.split(' ') {
        if flag.starts_with("-I") {
            let header_location = flag[2..].trim();
            header_locations.push(header_location);
        }
    }

    // Instruct pkg-config that we want to *statically* link DPDK. We will still have dynamic
    // dependencies on `libmlx5`, `libibverbs`, and `libnl`, though.
    let ldflags_bytes = Command::new("pkg-config")
        .env("PKG_CONFIG_PATH", &pkg_config_path)
        .args(&["--libs", "--static", "libdpdk"])
        .output()
        .unwrap_or_else(|e| panic!("Failed pkg-config ldflags: {:?}",e ))
        .stdout;
    let ldflags = String::from_utf8(ldflags_bytes).unwrap();

    // Step 1: Point Cargo to DPDK's libraries for linkage.
    // Cargo has linkage in "--as-needed" mode by default, which doesn't match up with DPDK's 
    // expectations: We need to include library dependencies without symbol dependencies since the
    // driver libraries rely on constructors to register themselves at runtime.
    println!("cargo:rustc-link-arg=-Wl,--no-as-needed");
    for flag in ldflags.split(' ') {
        if flag.starts_with("-L") {
            let library_location = &flag[2..];
            println!("cargo:rustc-link-search=native={}", library_location);
        } else if flag.starts_with("-l:lib") && flag.ends_with(".a") {
            let static_lib_name = &flag[6..flag.len()-2];
            println!("cargo:rustc-link-lib=static={}", static_lib_name);
        } else if flag.starts_with("-l") {
            let lib_name = &flag[2..];
            println!("cargo:rustc-link-lib={}", lib_name);
        } else if flag.starts_with("-Wl,") {
            println!("cargo:rustc-link-arg={}", flag);
        } else if flag == "-pthread" {
            continue;
        } else {
            panic!("Unrecognized build flag: {}", flag);
        }
    }

    // Link in `librte_net_mlx5` and its dependencies if desired.
    #[cfg(feature = "mlx5")] {
        println!("cargo:rustc-link-arg=-Wl,--no-as-needed");
        for static_lib_name in &["rte_net_mlx5", "rte_bus_pci", "rte_bus_vdev", "rte_common_mlx5"] {
            println!("cargo:rustc-link-lib=static={}", static_lib_name);
        }
    }

    // Reinstate Cargo's "environment"
    println!("cargo:rustc-link-arg=-Wl,--as-needed");

    // Step 2: Generate bindings for the DPDK headers.
    let mut builder = Builder::default();
    for header_location in &header_locations {
        builder = builder.clang_arg(&format!("-I{}", header_location));
    }
    let bindings = builder
        .blacklist_type("rte_arp_ipv4")
        .blacklist_type("rte_arp_hdr")

        .header("wrapper.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .generate()
        .unwrap_or_else(|e| panic!("Failed to generate bindings: {:?}", e));
    let bindings_out = out_dir.join("bindings.rs");
    bindings.write_to_file(bindings_out).expect("Failed to write bindings");

    // Step 3: Compile a stub file so Rust can access `inline` functions in the headers
    // that aren't compiled into the libraries.
    let mut builder = cc::Build::new();
    builder.opt_level(3);
    builder.pic(true);
    builder.flag("-march=native");
    builder.file("inlined.c");
    for header_location in &header_locations {
        builder.include(header_location);
    }
    builder.compile("inlined");
}
