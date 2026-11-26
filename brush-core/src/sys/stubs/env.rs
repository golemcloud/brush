//! Environment variable retrieval for platforms without a native implementation (such as WASI).

/// Retrieves environment variables from the host process.
///
/// WASI hands a component its environment like any process gets one, so this reads it through
/// the standard library.
pub(crate) fn get_host_env_vars() -> impl Iterator<Item = (String, String)> {
    // Names and values are byte strings, kept as the shell keeps bytes (see `rawbytes`).
    std::env::vars_os().map(|(k, v)| {
        (
            crate::rawbytes::from_os_str(&k).into_owned(),
            crate::rawbytes::from_os_str(&v).into_owned(),
        )
    })
}
