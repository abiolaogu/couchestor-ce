//! Build script for CoucheStor
//!
//! This script handles:
//! - Linking SPDK libraries when the `spdk` feature is enabled
//! - Linking Intel ISA-L libraries for erasure coding acceleration
//! - Setting up library search paths
//!
//! # Prerequisites
//!
//! ## SPDK Installation
//!
//! ```bash
//! # Clone and build SPDK
//! git clone https://github.com/spdk/spdk.git
//! cd spdk
//! git submodule update --init
//! ./configure --with-shared
//! make -j$(nproc)
//! sudo make install
//! sudo ldconfig
//! ```
//!
//! ## ISA-L Installation
//!
//! ```bash
//! # From package manager (recommended)
//! # Ubuntu/Debian:
//! sudo apt-get install libisal-dev
//!
//! # Fedora/RHEL:
//! sudo dnf install isa-l-devel
//!
//! # Or build from source:
//! git clone https://github.com/intel/isa-l.git
//! cd isa-l
//! ./autogen.sh
//! ./configure
//! make -j$(nproc)
//! sudo make install
//! sudo ldconfig
//! ```
//!
//! # Environment Variables
//!
//! - `SPDK_DIR` - Path to SPDK installation (default: /usr/local)
//! - `ISAL_DIR` - Path to ISA-L installation (default: /usr/local)
//! - `DPDK_DIR` - Path to DPDK installation (SPDK dependency)

use std::env;
use std::path::PathBuf;

fn main() {
    // Only run linking logic when spdk feature is enabled
    if !cfg!(feature = "spdk") {
        println!("cargo:warning=Building without SPDK support. Enable with --features spdk");
        return;
    }

    println!("cargo:rerun-if-env-changed=SPDK_DIR");
    println!("cargo:rerun-if-env-changed=ISAL_DIR");
    println!("cargo:rerun-if-env-changed=DPDK_DIR");

    // Get installation paths from environment or use defaults
    let spdk_dir = env::var("SPDK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/usr/local"));

    let isal_dir = env::var("ISAL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/usr/local"));

    let dpdk_dir = env::var("DPDK_DIR").map(PathBuf::from).ok();

    // =========================================================================
    // Library Search Paths
    // =========================================================================

    // SPDK library paths
    let spdk_lib_dir = spdk_dir.join("lib");
    if spdk_lib_dir.exists() {
        println!("cargo:rustc-link-search=native={}", spdk_lib_dir.display());
    }

    // Check for SPDK in lib64 (some distros use this)
    let spdk_lib64_dir = spdk_dir.join("lib64");
    if spdk_lib64_dir.exists() {
        println!(
            "cargo:rustc-link-search=native={}",
            spdk_lib64_dir.display()
        );
    }

    // ISA-L library paths
    let isal_lib_dir = isal_dir.join("lib");
    if isal_lib_dir.exists() {
        println!("cargo:rustc-link-search=native={}", isal_lib_dir.display());
    }

    // DPDK library paths (if specified)
    if let Some(ref dpdk) = dpdk_dir {
        let dpdk_lib = dpdk.join("lib");
        if dpdk_lib.exists() {
            println!("cargo:rustc-link-search=native={}", dpdk_lib.display());
        }
        let dpdk_lib64 = dpdk.join("lib64");
        if dpdk_lib64.exists() {
            println!("cargo:rustc-link-search=native={}", dpdk_lib64.display());
        }
    }

    // Standard system paths
    println!("cargo:rustc-link-search=native=/usr/lib");
    println!("cargo:rustc-link-search=native=/usr/lib64");
    println!("cargo:rustc-link-search=native=/usr/local/lib");
    println!("cargo:rustc-link-search=native=/usr/local/lib64");

    // =========================================================================
    // Intel ISA-L Libraries
    // =========================================================================

    // ISA-L erasure coding library (required)
    // This provides: ec_encode_data, gf_gen_cauchy1_matrix, etc.
    println!("cargo:rustc-link-lib=isal");

    // =========================================================================
    // SPDK Libraries
    // =========================================================================

    // Core SPDK libraries for memory management
    // These provide: spdk_dma_malloc, spdk_dma_free, etc.

    // SPDK environment abstraction layer
    println!("cargo:rustc-link-lib=spdk_env_dpdk");

    // SPDK utility library (includes DMA allocation)
    println!("cargo:rustc-link-lib=spdk_util");

    // SPDK logging
    println!("cargo:rustc-link-lib=spdk_log");

    // SPDK JSON for configuration
    println!("cargo:rustc-link-lib=spdk_json");

    // SPDK thread library
    println!("cargo:rustc-link-lib=spdk_thread");

    // =========================================================================
    // DPDK Libraries (SPDK dependency)
    // =========================================================================

    // Core DPDK libraries
    println!("cargo:rustc-link-lib=rte_eal");
    println!("cargo:rustc-link-lib=rte_mempool");
    println!("cargo:rustc-link-lib=rte_ring");
    println!("cargo:rustc-link-lib=rte_malloc");

    // =========================================================================
    // System Libraries
    // =========================================================================

    // NUMA support (for memory locality)
    println!("cargo:rustc-link-lib=numa");

    // UUID library (SPDK uses this for identifiers)
    println!("cargo:rustc-link-lib=uuid");

    // pthreads
    println!("cargo:rustc-link-lib=pthread");

    // dlopen support
    println!("cargo:rustc-link-lib=dl");

    // =========================================================================
    // Compiler Flags
    // =========================================================================

    // Enable large file support
    println!("cargo:rustc-link-arg=-D_LARGEFILE64_SOURCE");

    // Link as C++ for some SPDK components
    // println!("cargo:rustc-link-lib=stdc++");

    // =========================================================================
    // Verification
    // =========================================================================

    // Print configuration for debugging
    println!("cargo:warning=SPDK_DIR: {}", spdk_dir.display());
    println!("cargo:warning=ISAL_DIR: {}", isal_dir.display());
    if let Some(ref dpdk) = dpdk_dir {
        println!("cargo:warning=DPDK_DIR: {}", dpdk.display());
    }
}

// =============================================================================
// Alternative: pkg-config based discovery
// =============================================================================

/// Use pkg-config to find libraries (more portable).
///
/// Enable with: cargo build --features "spdk,pkg-config"
#[cfg(all(feature = "spdk", feature = "pkg-config"))]
fn find_with_pkg_config() {
    use pkg_config::Config;

    // Try to find SPDK via pkg-config
    if let Ok(lib) = Config::new().atleast_version("21.0").probe("spdk") {
        for path in lib.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
    }

    // Try to find ISA-L via pkg-config
    if let Ok(lib) = Config::new().atleast_version("2.0").probe("libisal") {
        for path in lib.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
    }
}
