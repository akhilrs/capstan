//! The trust store a run uses, as a seam: a test names its own store; a run without one uses the current user's.
use std::ffi::OsString;

use capstan_config::TrustStore;

/// Which trust store a run reads. The default is the current user's store.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustSeam {
    pub store: Option<TrustStore>,
}

impl TrustSeam {
    /// The store to use: the one set, else the current user's store, with `CAPSTAN_TRUST_DIR` read from `env` (the
    /// environment of the run, never the process's own).
    pub fn store(&self, env: &[(OsString, OsString)]) -> TrustStore {
        match &self.store {
            Some(store) => store.clone(),
            None => TrustStore::for_user_dir(
                crate::agent::var(env, "CAPSTAN_TRUST_DIR").map(OsString::from),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }

    #[test]
    fn a_set_store_is_returned_as_set() {
        let seam = TrustSeam {
            store: Some(TrustStore::at("/some/store")),
        };
        let with_dir = env(&[("CAPSTAN_TRUST_DIR", "/other")]);
        assert_eq!(seam.store(&with_dir), TrustStore::at("/some/store"));
        assert_eq!(seam.store(&[]), TrustStore::at("/some/store"));
    }

    #[test]
    fn no_store_reads_the_trust_dir_of_the_given_env() {
        let seam = TrustSeam::default();
        let with_dir = env(&[("CAPSTAN_TRUST_DIR", "/trust")]);
        assert_eq!(
            seam.store(&with_dir),
            TrustStore::for_user_with(|_| None, Some(OsString::from("/trust")))
        );
        let relative = env(&[("CAPSTAN_TRUST_DIR", "rel")]);
        assert_eq!(
            seam.store(&relative),
            TrustStore::for_user_with(|_| None, Some(OsString::from("rel")))
        );
    }
}
