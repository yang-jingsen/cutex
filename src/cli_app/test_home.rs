use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use uuid::Uuid;

static TEST_ENVIRONMENT_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
static TEST_ENVIRONMENT_LOCK: TestEnvironmentLock = TestEnvironmentLock;

pub(crate) struct TestEnvironmentLock;

pub(crate) struct TestEnvironmentGuard {
    previous_home: Option<OsString>,
    previous_private_test_home: Option<OsString>,
    _mutex_guard: MutexGuard<'static, ()>,
}

impl TestEnvironmentLock {
    pub(crate) fn lock(&self) -> Result<TestEnvironmentGuard, &'static str> {
        let mutex_guard = TEST_ENVIRONMENT_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(TestEnvironmentGuard {
            previous_home: std::env::var_os("HOME"),
            previous_private_test_home: std::env::var_os("CUTEX_TEST_PRIVATE_HOME"),
            _mutex_guard: mutex_guard,
        })
    }
}

impl Drop for TestEnvironmentGuard {
    fn drop(&mut self) {
        unsafe {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match self.previous_private_test_home.take() {
                Some(value) => std::env::set_var("CUTEX_TEST_PRIVATE_HOME", value),
                None => std::env::remove_var("CUTEX_TEST_PRIVATE_HOME"),
            }
        }
    }
}

pub(crate) fn environment_lock() -> &'static TestEnvironmentLock {
    &TEST_ENVIRONMENT_LOCK
}

pub(crate) struct IsolatedTestHome {
    root: PathBuf,
    remove_root_on_drop: bool,
    _environment_guard: TestEnvironmentGuard,
}

impl IsolatedTestHome {
    pub(crate) fn new(prefix: &str) -> std::io::Result<Self> {
        let environment_guard = environment_lock()
            .lock()
            .expect("shared test environment lock is infallible");
        let root = std::env::temp_dir().join(format!(
            "{prefix}-{}",
            &Uuid::new_v4().simple().to_string()[..8]
        ));

        create_owner_only_dir(&root)?;
        fs::write(root.join(".cutex-test-private-home"), b"")?;
        unsafe {
            std::env::set_var("HOME", &root);
            std::env::set_var("CUTEX_TEST_PRIVATE_HOME", &root);
        }

        Ok(Self {
            root,
            remove_root_on_drop: true,
            _environment_guard: environment_guard,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn retain_root(&mut self) {
        self.remove_root_on_drop = false;
    }
}

impl Drop for IsolatedTestHome {
    fn drop(&mut self) {
        if self.remove_root_on_drop {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[cfg(unix)]
fn create_owner_only_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_owner_only_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_home_restores_environment_and_removes_root() {
        let (previous_home, previous_private_test_home, root) = {
            let home = IsolatedTestHome::new("cth").expect("create isolated HOME");
            let previous_home = home._environment_guard.previous_home.clone();
            let previous_private_test_home =
                home._environment_guard.previous_private_test_home.clone();
            assert_eq!(
                std::env::var_os("HOME").as_deref(),
                Some(home.root().as_os_str())
            );
            assert_eq!(
                std::env::var_os("CUTEX_TEST_PRIVATE_HOME").as_deref(),
                Some(home.root().as_os_str())
            );
            assert_eq!(
                cutex::config::paths::home_dir().as_deref(),
                Some(home.root())
            );
            assert!(home.root().is_dir());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                assert_eq!(
                    fs::metadata(home.root())
                        .expect("isolated HOME metadata")
                        .permissions()
                        .mode()
                        & 0o777,
                    0o700
                );
            }
            (
                previous_home,
                previous_private_test_home,
                home.root().to_path_buf(),
            )
        };

        let _check_guard = environment_lock()
            .lock()
            .expect("shared test environment lock is infallible");
        assert_eq!(std::env::var_os("HOME"), previous_home);
        assert_eq!(
            std::env::var_os("CUTEX_TEST_PRIVATE_HOME"),
            previous_private_test_home
        );
        assert!(!root.exists());
    }

    #[test]
    fn isolated_home_restores_environment_and_removes_root_after_panic() {
        let mut previous_home = None;
        let mut previous_private_test_home = None;
        let mut root = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let home = IsolatedTestHome::new("ctp").expect("create isolated HOME");
            previous_home = Some(home._environment_guard.previous_home.clone());
            previous_private_test_home =
                Some(home._environment_guard.previous_private_test_home.clone());
            root = Some(home.root().to_path_buf());
            panic!("exercise isolated HOME unwind");
        }));

        assert!(result.is_err());
        let _check_guard = environment_lock()
            .lock()
            .expect("shared test environment lock is infallible");
        assert_eq!(std::env::var_os("HOME"), previous_home.flatten());
        assert_eq!(
            std::env::var_os("CUTEX_TEST_PRIVATE_HOME"),
            previous_private_test_home.flatten()
        );
        assert!(!root.expect("panic fixture root was recorded").exists());
    }

    #[test]
    fn private_test_home_without_marker_fails_closed() {
        let environment_guard = environment_lock()
            .lock()
            .expect("shared test environment lock is infallible");
        let root = std::env::temp_dir().join(format!(
            "cutex-unmarked-test-home-{}",
            &Uuid::new_v4().simple().to_string()[..8]
        ));
        create_owner_only_dir(&root).expect("create unmarked test HOME");
        unsafe {
            std::env::set_var("HOME", &root);
            std::env::set_var("CUTEX_TEST_PRIVATE_HOME", &root);
        }

        assert_eq!(cutex::config::paths::home_dir(), None);

        drop(environment_guard);
        fs::remove_dir(root).expect("remove unmarked test HOME");
    }
}
