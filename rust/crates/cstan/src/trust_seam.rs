//! The trust store a run uses, as a seam: a test names its own store; a run without one uses the current user's.
use std::ffi::OsString;
use std::path::PathBuf;

use capstan_config::TrustStore;

/// Which trust store a run reads. The default is the current user's store.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustSeam {
    pub store: Option<TrustStore>,
}

impl TrustSeam {
    /// The store to use: the one set, else the current user's (`TrustStore::for_user`).
    pub fn store(&self) -> TrustStore {
        match &self.store {
            Some(store) => store.clone(),
            None => TrustStore::for_user(),
        }
    }

    /// `store` with the passwd lookup and the `CAPSTAN_TRUST_DIR` value given (tests).
    pub fn store_with(
        &self,
        lookup: impl Fn(libc::uid_t) -> Option<PathBuf>,
        trust_dir: Option<OsString>,
    ) -> TrustStore {
        match &self.store {
            Some(store) => store.clone(),
            None => TrustStore::for_user_with(lookup, trust_dir),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_store_is_returned_as_set() {
        let seam = TrustSeam {
            store: Some(TrustStore::at("/some/store")),
        };
        assert_eq!(seam.store(), TrustStore::at("/some/store"));
        assert_eq!(
            seam.store_with(|_| None, Some(OsString::from("/other"))),
            TrustStore::at("/some/store")
        );
    }

    #[test]
    fn no_store_gives_the_for_user_store() {
        let seam = TrustSeam::default();
        let lookup = |_: libc::uid_t| Some(PathBuf::from("/home/x"));
        assert_eq!(
            seam.store_with(lookup, None),
            TrustStore::for_user_with(lookup, None)
        );
        let dir = Some(OsString::from("/trust"));
        assert_eq!(
            seam.store_with(lookup, dir.clone()),
            TrustStore::for_user_with(lookup, dir)
        );
        // `store` is `for_user` when nothing is set.
        assert_eq!(seam.store(), TrustStore::for_user());
    }
}
