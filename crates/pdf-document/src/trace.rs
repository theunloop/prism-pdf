//! Crate-internal logging shim (DESIGN.md §7): the `log_*` macros forward to `tracing` under the
//! `tracing` feature and compile to a type-checked no-op otherwise, so call sites carry no `#[cfg]`
//! noise and the un-instrumented build cannot rot. Format-string style only — the no-op arm is
//! `format_args!`, which does not understand `tracing`'s structured-field syntax — and the no-op
//! arm still evaluates arguments, so keep them cheap (offsets, object numbers, counts). **Never
//! log document content**: PDFs are user data, and a log line must not leak it.

#[cfg(feature = "tracing")]
macro_rules! log_warn {
    ($($arg:tt)*) => { tracing::warn!($($arg)*) };
}
#[cfg(not(feature = "tracing"))]
macro_rules! log_warn {
    ($($arg:tt)*) => {{ let _ = ::core::format_args!($($arg)*); }};
}
pub(crate) use log_warn;
