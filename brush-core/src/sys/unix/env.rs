//! Environment variable retrieval for Unix platforms.

/// Retrieves environment variables from the host process.
pub(crate) fn get_host_env_vars() -> impl Iterator<Item = (String, String)> {
    // Names and values are byte strings, kept as the shell keeps bytes (see `rawbytes`).
    std::env::vars_os().map(|(k, v)| {
        (
            crate::rawbytes::from_os_str(&k).into_owned(),
            crate::rawbytes::from_os_str(&v).into_owned(),
        )
    })
}
