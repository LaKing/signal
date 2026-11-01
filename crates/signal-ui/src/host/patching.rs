//! `patch` (M2.9-3): structural edits of the project's files, and the
//! presentation files that follow them.

/// ADR 0037, editable insides: what a patch's other files that could not be
/// put back say (`"; composites/x.toml could not be put back (…): it holds
/// the patch"`), empty when every one was.
pub(super) fn unrestored(results: &[(String, std::io::Result<bool>)]) -> String {
    results
        .iter()
        .filter_map(|(path, result)| match result {
            Ok(true) => None,
            Ok(false) => Some(format!(
                "; {path} could not be put back (another writer changed it meanwhile; its edit is kept)"
            )),
            Err(error) => Some(format!(
                "; {path} could not be put back ({error}): it holds the patch"
            )),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR 0037, editable insides: a failed put-back is said, never "nothing changed".
    #[test]
    fn a_side_file_not_put_back_is_said() {
        assert_eq!(
            unrestored(&[("composites/a.toml".to_owned(), Ok(true))]),
            ""
        );
        let said = unrestored(&[
            ("composites/a.toml".to_owned(), Ok(false)),
            (
                "composites/b.toml".to_owned(),
                Err(std::io::Error::other("disk full")),
            ),
        ]);
        assert_eq!(
            said,
            "; composites/a.toml could not be put back (another writer changed it meanwhile; its edit is kept); composites/b.toml could not be put back (disk full): it holds the patch"
        );
    }
}
