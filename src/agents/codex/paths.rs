use std::path::{Path, PathBuf};

pub fn codex_home(home: &Path) -> PathBuf {
    #[cfg(test)]
    let _lock = test_lock::acquire();

    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
}

#[cfg(test)]
pub(crate) mod test_lock {
    use std::cell::Cell;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::{LazyLock, Mutex, MutexGuard};

    static LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    thread_local! {
        static DEPTH: Cell<usize> = const { Cell::new(0) };
    }

    pub struct Guard(Option<MutexGuard<'static, ()>>);

    pub fn acquire() -> Guard {
        if DEPTH.with(|depth| depth.get()) > 0 {
            return Guard(None);
        }
        let guard = LOCK.lock().unwrap_or_else(|err| err.into_inner());
        DEPTH.with(|depth| depth.set(depth.get() + 1));
        Guard(Some(guard))
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if self.0.take().is_some() {
                DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
            }
        }
    }

    pub struct CodexHomeOverride {
        _lock: Guard,
        previous: Option<OsString>,
    }

    impl CodexHomeOverride {
        pub fn set(value: &Path) -> Self {
            let lock = acquire();
            let previous = std::env::var_os("CODEX_HOME");
            std::env::set_var("CODEX_HOME", value);
            Self {
                _lock: lock,
                previous,
            }
        }
    }

    impl Drop for CodexHomeOverride {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(previous) => std::env::set_var("CODEX_HOME", previous),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }
    }
}
