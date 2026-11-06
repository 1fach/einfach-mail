//! Prevents other desktop processes from bypassing in-memory cache reservations.

use std::{
    fs::{File, OpenOptions, Permissions, TryLockError},
    io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

pub(super) struct ProcessLock {
    _file: File,
}

impl ProcessLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        file.set_permissions(Permissions::from_mode(0o600))?;

        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Einfach Mail is already running for this Linux user",
            )),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }
}

pub(super) fn run_with_user_lock<T>(run_application: impl FnOnce() -> T) -> io::Result<T> {
    let lock_path =
        xdg::BaseDirectories::with_prefix("einfach-mail").place_data_file("instance.lock")?;
    run_with_process_lock(&lock_path, run_application)
}

fn run_with_process_lock<T>(path: &Path, run_application: impl FnOnce() -> T) -> io::Result<T> {
    let _process_lock = ProcessLock::acquire(path)?;
    Ok(run_application())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        process::{Command, Stdio},
        thread,
        time::Duration,
    };

    use uuid::Uuid;

    use super::{ProcessLock, run_with_process_lock};

    fn hold_instance_lock_when_running_as_child() -> bool {
        let Some(lock_path) = std::env::var_os("EINFACH_MAIL_TEST_LOCK_PATH") else {
            return false;
        };
        let lock_path = std::path::PathBuf::from(lock_path);
        let release_path = lock_path.with_extension("release");
        let ready_path = lock_path.with_extension("ready");
        let _process_lock = ProcessLock::acquire(&lock_path)
            .expect("child process acquires the synthetic instance lock");
        fs::write(ready_path, b"ready").expect("child process announces lock ownership");
        for _ in 0..2_000 {
            if release_path.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        true
    }

    #[test]
    fn process_lock_rejects_competing_startup_and_releases_after_exit() {
        if hold_instance_lock_when_running_as_child() {
            return;
        }

        let directory =
            std::env::temp_dir().join(format!("einfach-mail-process-lock-{}", Uuid::new_v4()));
        fs::create_dir(&directory).expect("synthetic lock directory is created");
        let lock_path = directory.join("instance.lock");
        let ready_path = lock_path.with_extension("ready");
        let release_path = lock_path.with_extension("release");
        let test_executable = std::env::current_exe().expect("test executable path is available");
        let mut child_process = Command::new(test_executable)
            .args([
                "--exact",
                "single_instance::tests::process_lock_rejects_competing_startup_and_releases_after_exit",
                "--nocapture",
            ])
            .env("EINFACH_MAIL_TEST_LOCK_PATH", &lock_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("synthetic child process starts");

        let mut child_acquired_lock = false;
        for _ in 0..2_000 {
            if ready_path.exists() {
                child_acquired_lock = true;
                break;
            }
            if child_process
                .try_wait()
                .expect("child process status can be read")
                .is_some()
            {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }

        let mut duplicate_startup_called = false;
        let duplicate_result = run_with_process_lock(&lock_path, || {
            duplicate_startup_called = true;
        });
        fs::write(&release_path, b"release").expect("child process can release the test lock");
        let child_status = child_process.wait().expect("child process exits");

        let mut startup_after_exit_called = false;
        run_with_process_lock(&lock_path, || {
            startup_after_exit_called = true;
        })
        .expect("a new process acquires the lock after the old process exits");

        fs::remove_dir_all(directory).expect("synthetic lock directory is removed");

        assert!(child_acquired_lock, "child process acquired the lock");
        assert!(child_status.success(), "child process exited successfully");
        assert!(matches!(
            duplicate_result,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert!(!duplicate_startup_called);
        assert!(startup_after_exit_called);
    }
}
