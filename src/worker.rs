//! A persistent JSON-lines bridge per camera. The factory owns worker lifetimes.
use moqui_core::web::StreamCancellation;
use moqui_lib::error::Error;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

type CameraKey = (String, String);
#[derive(Default)]
pub struct WorkerPool(Mutex<HashMap<CameraKey, Arc<Mutex<Worker>>>>, AtomicBool);

impl WorkerPool {
    pub fn exchange(
        &self,
        key: CameraKey,
        python: &Path,
        script: &Path,
        payload: &serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, Error> {
        self.exchange_cancellable(key, python, script, payload, timeout, None)
    }

    pub fn exchange_cancellable(
        &self,
        key: CameraKey,
        python: &Path,
        script: &Path,
        payload: &serde_json::Value,
        timeout: Duration,
        cancellation: Option<&StreamCancellation>,
    ) -> Result<serde_json::Value, Error> {
        let deadline = Instant::now() + timeout;
        let cancelled = || cancellation.is_some_and(StreamCancellation::is_cancelled);
        if cancelled() {
            return Err(Error::new("GenICam capture cancelled"));
        }
        let worker = {
            let mut workers = self
                .0
                .lock()
                .map_err(|_| Error::new("GenICam worker pool lock poisoned"))?;
            if self.1.load(Ordering::Acquire) {
                return Err(Error::new("GenICam factory is shutting down"));
            }
            if !workers.contains_key(&key) {
                workers.insert(
                    key.clone(),
                    Arc::new(Mutex::new(Worker::spawn(python, script)?)),
                );
            }
            workers[&key].clone()
        };
        let mut bridge = loop {
            if cancelled() {
                return Err(Error::new(
                    "GenICam capture cancelled while waiting for camera",
                ));
            }
            if Instant::now() >= deadline {
                return Err(Error::new("GenICam camera busy/timed out"));
            }
            match worker.try_lock() {
                Ok(bridge) => break bridge,
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(Error::new("GenICam worker lock poisoned"));
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    if let Some(cancellation) = cancellation {
                        cancellation.wait_timeout(Duration::from_millis(10));
                    } else {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        };
        let result = bridge.exchange(
            payload,
            deadline.saturating_duration_since(Instant::now()),
            cancellation,
        );
        if result.is_err() {
            // Do not replay writes/triggers after an uncertain failure.
            bridge.stop();
            if let Ok(mut workers) = self.0.lock()
                && workers
                    .get(&key)
                    .is_some_and(|current| Arc::ptr_eq(current, &worker))
            {
                workers.remove(&key);
            }
        }
        result
    }

    pub fn close(&self, key: &CameraKey) {
        let worker = self
            .0
            .lock()
            .ok()
            .and_then(|mut workers| workers.remove(key));
        if let Some(worker) = worker
            && let Ok(mut bridge) = worker.lock()
        {
            bridge.close();
        }
    }

    pub fn close_all(&self) {
        self.1.store(true, Ordering::Release);
        let workers: Vec<_> = self
            .0
            .lock()
            .map(|mut workers| workers.drain().map(|(_, worker)| worker).collect())
            .unwrap_or_default();
        for worker in workers {
            if let Ok(mut bridge) = worker.lock() {
                bridge.close();
            }
        }
    }
}

struct Worker {
    child: Child,
    input: ChildStdin,
    responses: mpsc::Receiver<Result<String, std::io::Error>>,
}

impl Worker {
    fn spawn(python: &Path, script: &Path) -> Result<Self, Error> {
        let mut child = Command::new(python)
            .arg("-u")
            .arg(script)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| Error::new(format!("Cannot start GenICam worker: {e}")))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| Error::new("worker stdin unavailable"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| Error::new("worker stdout unavailable"))?;
        let (send, responses) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            input,
            responses,
        })
    }

    fn exchange(
        &mut self,
        payload: &serde_json::Value,
        timeout: Duration,
        cancellation: Option<&StreamCancellation>,
    ) -> Result<serde_json::Value, Error> {
        serde_json::to_writer(&mut self.input, payload)
            .map_err(|e| Error::new(format!("worker request failed: {e}")))?;
        self.input
            .write_all(b"\n")
            .and_then(|_| self.input.flush())
            .map_err(|e| Error::new(format!("worker stdin failed: {e}")))?;
        let deadline = Instant::now() + timeout;
        let response = loop {
            if cancellation.is_some_and(StreamCancellation::is_cancelled) {
                return Err(Error::new("GenICam capture cancelled during acquisition"));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::new("GenICam worker response timed out"));
            }
            match self
                .responses
                .recv_timeout(remaining.min(Duration::from_millis(20)))
            {
                Ok(response) => {
                    break response.map_err(|e| Error::new(format!("worker stdout failed: {e}")))?;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(error) => {
                    return Err(Error::new(format!(
                        "GenICam worker response failed: {error}"
                    )));
                }
            }
        };
        serde_json::from_str(&response)
            .map_err(|e| Error::new(format!("Invalid GenICam worker JSON: {e}")))
    }

    fn close(&mut self) {
        let _ = self.exchange(
            &serde_json::json!({"command":"close-all"}),
            Duration::from_secs(3),
            None,
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_worker_is_terminated_without_replaying_request() -> Result<(), Error> {
        let script =
            std::env::temp_dir().join(format!("moqui-genicam-stall-{}.py", std::process::id()));
        std::fs::write(
            &script,
            "import sys, time\nsys.stdin.readline()\ntime.sleep(30)\n",
        )
        .map_err(|e| Error::new(e.to_string()))?;
        let pool = WorkerPool::default();
        let result = pool.exchange(
            ("mock".into(), "stalled".into()),
            Path::new("python3"),
            &script,
            &serde_json::json!({"command":"run-action"}),
            Duration::from_millis(100),
        );
        std::fs::remove_file(script).map_err(|e| Error::new(e.to_string()))?;
        assert!(result.is_err());
        assert!(pool.0.lock().unwrap().is_empty());
        Ok(())
    }

    #[test]
    fn cancellation_interrupts_acquisition_and_retires_the_worker() -> Result<(), Error> {
        let prefix =
            std::env::temp_dir().join(format!("moqui-genicam-cancel-{}", std::process::id()));
        let script = prefix.with_extension("py");
        let marker = prefix.with_extension("started");
        std::fs::write(&script, format!("import sys, time, pathlib\nsys.stdin.readline()\npathlib.Path({:?}).write_text('one request')\ntime.sleep(30)\n", marker.to_string_lossy()))
            .map_err(|e| Error::new(e.to_string()))?;
        let pool = Arc::new(WorkerPool::default());
        let worker_pool = pool.clone();
        let cancellation = StreamCancellation::default();
        let signal = cancellation.clone();
        let worker_script = script.clone();
        let (send, done) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let result = worker_pool.exchange_cancellable(
                ("mock".into(), "cancel".into()),
                Path::new("python3"),
                &worker_script,
                &serde_json::json!({"command":"run-action"}),
                Duration::from_secs(30),
                Some(&signal),
            );
            let _ = send.send(result);
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        cancellation.cancel();
        let result = done
            .recv_timeout(Duration::from_secs(2))
            .expect("cancelled acquisition must stop promptly");
        thread.join().unwrap();
        assert!(
            marker.exists(),
            "fixture must reach its blocking acquisition"
        );
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(pool.0.lock().unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "one request");
        std::fs::remove_file(script).unwrap();
        std::fs::remove_file(marker).unwrap();
        Ok(())
    }

    #[test]
    fn mock_camera_keeps_frame_cache_across_calls() -> Result<(), Error> {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("script/genicam_bridge.py");
        let pool = WorkerPool::default();
        let key = ("mock".into(), "MOCK_CAMERA".into());
        let payload = serde_json::json!({"command":"run-action", "cti_path":"mock", "serial_number":"MOCK_CAMERA", "action":"visual_servo_frame", "parameters_map":{"use_cached":false}});
        let first = pool.exchange(
            key.clone(),
            Path::new("python3"),
            &script,
            &payload,
            Duration::from_secs(15),
        )?;
        let mut second_payload = payload.clone();
        second_payload["parameters_map"]["use_cached"] = true.into();
        let second = pool.exchange(
            key.clone(),
            Path::new("python3"),
            &script,
            &second_payload,
            Duration::from_secs(15),
        )?;
        assert_eq!(first["source"], "capture");
        assert_eq!(second["source"], "cache");
        assert_eq!(first["captured_at"], second["captured_at"]);
        assert!(second["frame_bytes"]["__base64__"].as_str().is_some());
        pool.close(&key);
        // Closing a single camera permits a later capture. Factory shutdown does not.
        pool.exchange(
            key.clone(),
            Path::new("python3"),
            &script,
            &payload,
            Duration::from_secs(15),
        )?;
        pool.close_all();
        let error = pool
            .exchange(
                key,
                Path::new("python3"),
                &script,
                &payload,
                Duration::from_secs(15),
            )
            .unwrap_err();
        assert!(error.to_string().contains("shutting down"));
        assert!(pool.0.lock().unwrap().is_empty());
        Ok(())
    }
}
