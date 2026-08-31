pub mod matcher;
pub mod model;

// The on-disk index cache is Linux-specific (Linux launches fresh each time and
// wants a <2ms cold start). Windows enumerates the Start Menu on demand.
#[cfg(target_os = "linux")]
pub mod indexer;
