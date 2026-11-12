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

//! The tool `GenicamMultiCameraRunner`: the Rust port of the Groovy class `org.moqui.genicam.GenicamMultiCameraRunner`.
//! `acquireMultiVideoFile(cameraList, options)` runs `script/multi_camera_capture.py multi-video-file` with the
//! request on stdin, as the class did with a ProcessBuilder; the script starts a worker process per camera, so the
//! SDK state of each camera is in a process of its own. The process is stopped after `options.processTimeoutMs`
//! (else `genicam.multi.process.timeout.ms`, else 120 s) and the capture is not tried again. A failing process with
//! a JSON report gives the report with `exit_code` and `stderr`.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{Tool, ToolFactory};
use moqui_lib::object::Value;

use crate::client::{bridge_value, component_script, property_number};
use crate::worker::{python_executable, script_directory};

const CAPTURE_SCRIPT: &str = "component://moqui-genicam/script/multi_camera_capture.py";

#[derive(Default)]
pub struct GenicamMultiCameraRunnerFactory;

impl ToolFactory for GenicamMultiCameraRunnerFactory {
    fn name(&self) -> &str {
        "GenicamMultiCameraRunner"
    }

    fn get_instance(&self, _parameters: &[Value]) -> Result<Arc<dyn Tool>, Error> {
        Ok(Arc::new(GenicamMultiCameraRunner))
    }

    fn type_declaration(&self) -> Option<&'static str> {
        Some(
            "{ acquireMultiVideoFile(cameraList: Record<string, unknown>[], options: Record<string, unknown>): \
             { results: unknown[]; successful_count: number; failed_count: number } }",
        )
    }
}

struct GenicamMultiCameraRunner;

impl Tool for GenicamMultiCameraRunner {
    fn methods(&self) -> &'static [&'static str] {
        &["acquireMultiVideoFile"]
    }

    fn call(&self, ec: &ExecutionContext, method: &str, arguments: &[Value]) -> Result<Value, Error> {
        if method != "acquireMultiVideoFile" {
            return Err(Error::new(format!(
                "The GenicamMultiCameraRunner tool has no method {method}"
            )));
        }
        let cameras = arguments.first().map(moqui_lib::json::to_json).unwrap_or_default();
        if cameras.as_array().is_none_or(Vec::is_empty) {
            return Err(Error::new("At least one camera configuration is required."));
        }
        let options = arguments
            .get(1)
            .map(moqui_lib::json::to_json)
            .filter(serde_json::Value::is_object);
        let options = options.unwrap_or_else(|| serde_json::json!({}));
        let timeout = options
            .get("processTimeoutMs")
            .and_then(serde_json::Value::as_u64)
            .or_else(|| property_number("genicam.multi.process.timeout.ms"))
            .filter(|ms| *ms > 0)
            .unwrap_or(120_000);
        let request = serde_json::json!({"command": "multi-video-file", "options": options, "cameras": cameras});
        let runtime_dir = ec.factory().runtime_dir().to_path_buf();
        let python = python_executable(&runtime_dir).map_err(|e| Error::new(e.to_string()))?;
        let directory = script_directory(&runtime_dir);
        run(
            &python,
            &component_script(ec, CAPTURE_SCRIPT)?,
            &directory,
            &request,
            Duration::from_millis(timeout),
        )
    }
}

fn run(
    python: &PathBuf,
    script: &PathBuf,
    directory: &PathBuf,
    request: &serde_json::Value,
    timeout: Duration,
) -> Result<Value, Error> {
    let mut child = Command::new(python)
        .arg(script)
        .arg("multi-video-file")
        .current_dir(directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            Error::new(format!(
                "Cannot start the GenICam multi-camera process {}: {e}",
                python.display()
            ))
        })?;
    let sent = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("no stdin"))
        .and_then(|mut stdin| stdin.write_all(request.to_string().as_bytes()));
    if let Err(error) = sent {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Error::new(format!("Cannot send the multi-camera request: {error}")));
    }
    // both pipes are read while the process runs, so a long report cannot block it
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).trim().to_string()
        })
    };
    let stdout = drain(child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| Error::new(e.to_string()))? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::new(format!(
                "GenICam multi-camera process timed out after {} ms.",
                timeout.as_millis()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if !stderr.is_empty() {
        tracing::info!("GenICam multi-camera stderr: {stderr}");
    }
    let report = serde_json::from_str::<serde_json::Value>(&stdout)
        .ok()
        .filter(serde_json::Value::is_object);
    match (status.success(), report) {
        (true, Some(report)) => bridge_value(&report),
        (true, None) => Err(Error::new(format!(
            "Unexpected GenICam multi-camera response: {stdout}"
        ))),
        (false, Some(mut report)) => {
            report["exit_code"] = status.code().unwrap_or(-1).into();
            report["stderr"] = stderr.into();
            bridge_value(&report)
        }
        (false, None) => Err(Error::new(format!(
            "GenICam multi-camera process failed with exit code {:?}. stderr={stderr}",
            status.code()
        ))),
    }
}
