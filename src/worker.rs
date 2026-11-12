/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! The Python processes of `script/genicam_bridge.py`, in place of the JEP interpreter of the Java version: one
//! worker per camera (CTI and serial number), started at the first call and kept, so the camera connection, the
//! streaming thread and the latest frame live between calls as they did in the interpreter. A worker answers one
//! JSON line for each JSON line it gets. Calls to one camera wait for each other; calls to different cameras do not.
//!
//! A call that fails without an answer (a timeout, a broken pipe) stops the worker: its command is not sent again,
//! since a write or a trigger may have reached the camera. The next call starts a new worker.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::error::Error;

/// CTI path and serial number.
pub type CameraKey = (String, String);

/// How a worker process starts.
#[derive(Clone)]
pub struct Launch {
    pub python: PathBuf,
    pub script: PathBuf,
    /// The directory the bridge resolves its relative paths (runtime/genicam/...) from: the framework directory.
    pub directory: PathBuf,
}

#[derive(Default)]
pub struct WorkerPool {
    workers: Mutex<HashMap<CameraKey, Arc<Mutex<Worker>>>>,
    closed: AtomicBool,
}

impl WorkerPool {
    /// Send one request to the worker of the camera and wait up to `timeout` for its answer.
    pub fn exchange(
        &self,
        key: &CameraKey,
        launch: &Launch,
        request: &serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, Error> {
        let deadline = Instant::now() + timeout;
        let worker = {
            let mut workers = self
                .workers
                .lock()
                .map_err(|_| Error::new("The GenICam worker pool lock is poisoned"))?;
            if self.closed.load(Ordering::Acquire) {
                return Err(Error::new("The GenicamClient tool is shutting down"));
            }
            match workers.get(key) {
                Some(worker) => Arc::clone(worker),
                None => {
                    let worker = Arc::new(Mutex::new(Worker::spawn(launch)?));
                    workers.insert(key.clone(), Arc::clone(&worker));
                    worker
                }
            }
        };
        let mut bridge = loop {
            match worker.try_lock() {
                Ok(bridge) => break bridge,
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(Error::new("The GenICam worker lock is poisoned"))
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(Error::new(format!(
                            "The camera {} is busy: no answer within the timeout",
                            key.1
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        };
        let answer = bridge.exchange(request, deadline.saturating_duration_since(Instant::now()));
        if answer.is_err() {
            bridge.stop();
            if let Ok(mut workers) = self.workers.lock() {
                if workers.get(key).is_some_and(|current| Arc::ptr_eq(current, &worker)) {
                    workers.remove(key);
                }
            }
        }
        answer
    }

    /// Close the worker of a camera: its connection and streaming thread end. A later call starts a new one.
    pub fn close(&self, key: &CameraKey) {
        let worker = self.workers.lock().ok().and_then(|mut workers| workers.remove(key));
        if let Some(worker) = worker {
            if let Ok(mut bridge) = worker.lock() {
                bridge.close();
            }
        }
    }

    /// Close every worker; later calls are refused (the factory is shutting down).
    pub fn close_all(&self) {
        self.closed.store(true, Ordering::Release);
        let workers: Vec<_> = self
            .workers
            .lock()
            .map(|mut workers| workers.drain().map(|(_, worker)| worker).collect())
            .unwrap_or_default();
        for worker in workers {
            if let Ok(mut bridge) = worker.lock() {
                bridge.close();
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.workers.lock().map(|workers| workers.len()).unwrap_or_default()
    }
}

struct Worker {
    child: Child,
    input: ChildStdin,
    answers: mpsc::Receiver<Result<String, std::io::Error>>,
}

impl Worker {
    fn spawn(launch: &Launch) -> Result<Self, Error> {
        let mut command = Command::new(&launch.python);
        command.arg("-u").arg(&launch.script).arg("serve");
        if launch.directory.is_dir() {
            command.current_dir(&launch.directory);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                Error::new(format!(
                    "Cannot start the GenICam worker {}: {e}",
                    launch.python.display()
                ))
            })?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| Error::new("The GenICam worker has no stdin"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| Error::new("The GenICam worker has no stdout"))?;
        let (send, answers) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self { child, input, answers })
    }

    fn exchange(&mut self, request: &serde_json::Value, timeout: Duration) -> Result<serde_json::Value, Error> {
        serde_json::to_writer(&mut self.input, request)
            .map_err(|e| Error::new(format!("Cannot send the GenICam request: {e}")))?;
        self.input
            .write_all(b"\n")
            .and_then(|_| self.input.flush())
            .map_err(|e| Error::new(format!("Cannot send the GenICam request: {e}")))?;
        let line = match self.answers.recv_timeout(timeout) {
            Ok(line) => line.map_err(|e| Error::new(format!("Cannot read the GenICam answer: {e}")))?,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(Error::new(format!(
                    "No GenICam answer within {} ms",
                    timeout.as_millis()
                )))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Error::new("The GenICam worker ended")),
        };
        serde_json::from_str(&line).map_err(|e| Error::new(format!("The GenICam answer is not JSON: {e}")))
    }

    fn close(&mut self) {
        let _ = self.exchange(&serde_json::json!({"command": "close-all"}), Duration::from_secs(5));
        self.stop();
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The working directory of the Python scripts. They resolve their own relative paths (runtime/genicam/...: the mock
/// camera state, the latest frame file) from it, as they did from the JVM's directory: the directory that holds the
/// runtime directory when it is named runtime, else the runtime directory itself.
pub fn script_directory(runtime_dir: &Path) -> PathBuf {
    match runtime_dir.parent() {
        Some(parent) if runtime_dir.file_name().is_some_and(|name| name == "runtime") => parent.to_path_buf(),
        _ => runtime_dir.to_path_buf(),
    }
}

/// The Python interpreter: the runtime's python_venv as in the Java version, else the PYTHON environment variable,
/// else python3 or python on the PATH.
pub fn python_executable(runtime_dir: &Path) -> Result<PathBuf, Error> {
    let venv = if cfg!(windows) {
        runtime_dir.join("python_venv").join("Scripts").join("python.exe")
    } else {
        runtime_dir.join("python_venv").join("bin").join("python")
    };
    if venv.is_file() {
        return Ok(venv);
    }
    if let Some(python) = std::env::var_os("PYTHON")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Ok(python);
    }
    let names: &[&str] = if cfg!(windows) {
        &["python.exe", "python3.exe"]
    } else {
        &["python3", "python"]
    };
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            for name in names {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Err(Error::new(format!(
        "Python executable not found for moqui-genicam at {}, in PYTHON or on the PATH",
        venv.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(script: PathBuf) -> Launch {
        Launch {
            python: PathBuf::from("python3"),
            script,
            directory: std::env::temp_dir(),
        }
    }

    #[test]
    fn a_stalled_worker_is_stopped_without_sending_the_request_again() {
        let script = std::env::temp_dir().join(format!("moqui-genicam-stall-{}.py", std::process::id()));
        std::fs::write(&script, "import sys, time\nsys.stdin.readline()\ntime.sleep(30)\n").unwrap();
        let pool = WorkerPool::default();
        let answer = pool.exchange(
            &("mock".into(), "stalled".into()),
            &launch(script.clone()),
            &serde_json::json!({"action": "write"}),
            Duration::from_millis(200),
        );
        std::fs::remove_file(script).unwrap();
        assert!(answer.is_err());
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn the_mock_camera_keeps_its_latest_frame_between_calls() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("script/genicam_bridge.py");
        let pool = WorkerPool::default();
        let key: CameraKey = ("mock.cti".into(), "MOCK_CAMERA".into());
        let request = |use_cached: bool| {
            serde_json::json!({"action": "visual_servo_frame", "cti_path": "mock.cti", "serial_number": "MOCK_CAMERA",
                "parameters_map": {"use_cached": use_cached, "save_snapshot": false}})
        };
        let first = pool
            .exchange(&key, &launch(script.clone()), &request(false), Duration::from_secs(30))
            .unwrap();
        assert_eq!(first["source"], "capture");
        assert!(first["frame_bytes"]["__base64__"].as_str().is_some());
        pool.close(&key);
        // closing one camera allows a later call; closing the pool does not
        pool.exchange(&key, &launch(script.clone()), &request(false), Duration::from_secs(30))
            .unwrap();
        pool.close_all();
        let refused = pool
            .exchange(&key, &launch(script), &request(false), Duration::from_secs(30))
            .unwrap_err();
        assert!(refused.to_string().contains("shutting down"));
        assert_eq!(pool.len(), 0);
    }
}
