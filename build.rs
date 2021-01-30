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

    let mut library_location = None;
    let mut lib_names = vec![];
    let mut static_lib_names = vec![];
    for flag in ldflags.split(' ') {
        if flag.starts_with("-L") {
            assert_eq!(library_location, None);
            library_location = Some(&flag[2..]);
        } else if flag.starts_with("-l:lib") && flag.ends_with(".a") {
            static_lib_names.push(&flag[6..flag.len()-2]);
        } else if flag.starts_with("-l") {
            lib_names.push(&flag[2..]);
        }
    }

    // Link in `librte_net_mlx5` and its dependencies if desired.
    #[cfg(feature = "mlx5")] {
        static_lib_names.extend(&["rte_net_mlx5", "rte_bus_pci", "rte_bus_vdev", "rte_common_mlx5"]);
    }

    // Step 1: Now that we've compiled and installed DPDK, point cargo to the libraries.
    println!("cargo:rustc-link-search=native={}", library_location.unwrap());
    for lib_name in &lib_names {
        println!("cargo:rustc-link-lib={}", lib_name);
    }
    for lib_name in &static_lib_names {
        println!("cargo:rustc-link-lib=static={}", lib_name);
    }

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
