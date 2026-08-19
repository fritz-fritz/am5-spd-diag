//! AM5 DDR5 SPD hub diagnostics: SMBIOS, SMBus, capture, and reports.
//!
//! Capture/timeline/`hub.json` contracts are frozen in [`schema`] so CLI and
//! the GTK notify window share the same parsers and fixtures.

pub mod analyze;
pub mod capture;
pub mod config;
pub mod dimm;
pub mod hub;
pub mod i2c;
pub mod notify;
pub mod paths;
pub mod schema;
pub mod smbios;

pub use analyze::{
    build_context, fill_template, load_timeline, load_timeline_from_package, make_package,
    mapping_from_context, open_package, print_analyze, print_status, recover_cleared,
    recover_this_boot, render_report, write_report, PackageSession,
};
pub use dimm::{dimm_flags, format_dimm_summary, parse_dimm_summary, parse_dmidecode_memory};
pub use hub::format_probe_human;
pub use schema::{Baseline, HubProbe, TimelineEvent, FORUM_URL};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
