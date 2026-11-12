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

//! The tool `GenicamClient`: the Rust port of the Groovy class `org.moqui.genicam.GenicamClient`. The services of
//! GenicamServices.xml (through Genicam.js) take it as they took the class, for one camera:
//! `ec.getTool('GenicamClient', ctiPath, serialNumber, deviceId)`.
//!
//! The methods are the class's, with the arguments the services resolve (paths, formats, sizes):
//!
//! - `read(parameterNames)`, `write(parametersMap)`: the GenICam nodes of a DeviceRequest; the camera's values by
//!   node name. Storing them is the services' work (DeviceAcquisitionServices.store#Samples).
//! - `acquireSingleImage(outputDir, imageFormat, resizeWidth, resizeHeight)`
//! - `acquireVideoStream(numFrames, outputDir, imageFormat, resizeWidth, resizeHeight)`
//! - `acquireVideoFile(numFrames, fps, outputDir, videoContainer, videoCodec, resizeWidth, resizeHeight)`
//! - `acquire3dFrame()`, `getFrameBytes()`, `getFramePayload()`
//! - `acquireVisualServoFrame(useCachedFrame, saveSnapshot, outputDir, imageFormat, resizeWidth, resizeHeight)`
//! - `close()`: ends the camera's worker (connection, streaming thread, latest frame).
//! - `pause(millis)`: waits, as the Groovy live stream did with `Thread.sleep` between frames.
//!
//! The Java version ran `script/genicam_bridge.py` in a JEP interpreter; here it runs in a Python worker process per
//! camera (worker.rs), which the factory keeps until `close()` or shutdown. The answers keep the bridge's names; its
//! byte buffers become bytes. An answer with an error is an error of the call.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{Tool, ToolFactory};
use moqui_lib::object::Value;

use crate::worker::{python_executable, script_directory, CameraKey, Launch, WorkerPool};

const BRIDGE_SCRIPT: &str = "component://moqui-genicam/script/genicam_bridge.py";

#[derive(Default)]
pub struct GenicamClientFactory {
    pub(crate) workers: Arc<WorkerPool>,
}

impl ToolFactory for GenicamClientFactory {
    fn name(&self) -> &str {
        "GenicamClient"
    }

    fn get_instance(&self, parameters: &[Value]) -> Result<Arc<dyn Tool>, Error> {
        let text = |index: usize| {
            parameters
                .get(index)
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
        };
        if text(0).is_empty() {
            return Err(Error::new("GenicamClient needs the CTI path of the GenTL driver"));
        }
        Ok(Arc::new(GenicamClient {
            workers: Arc::clone(&self.workers),
            cti_path: text(0).to_string(),
            serial_number: text(1).to_string(),
            device_id: Some(text(2)).filter(|id| !id.is_empty()).map(str::to_string),
        }))
    }

    fn destroy(&self) {
        self.workers.close_all();
    }

    fn type_declaration(&self) -> Option<&'static str> {
        Some(
            "{ read(parameterNames: string[]): Record<string, unknown>; \
             write(parametersMap: Record<string, unknown>): Record<string, unknown>; \
             acquireSingleImage(outputDir: string, imageFormat: string, resizeWidth?: number | null, resizeHeight?: number | null): any; \
             acquireVideoStream(numFrames: number, outputDir: string, imageFormat: string, resizeWidth?: number | null, resizeHeight?: number | null): any; \
             acquireVideoFile(numFrames: number, fps: number, outputDir: string, videoContainer: string, videoCodec: string, resizeWidth?: number | null, resizeHeight?: number | null): any; \
             acquire3dFrame(): any; getFrameBytes(): Uint8Array; getFramePayload(): any; \
             acquireVisualServoFrame(useCachedFrame: boolean, saveSnapshot: boolean, outputDir: string, imageFormat: string, resizeWidth?: number | null, resizeHeight?: number | null): any; \
             close(): void; pause(millis: number): void; \
             readonly ctiPath: string; readonly serialNumber: string; readonly deviceId: string | null }",
        )
    }
}

pub struct GenicamClient {
    workers: Arc<WorkerPool>,
    cti_path: String,
    serial_number: String,
    device_id: Option<String>,
}

impl GenicamClient {
    fn key(&self) -> CameraKey {
        (self.cti_path.clone(), self.serial_number.clone())
    }

    /// `runAction` of the Groovy class: one action of the bridge, with the runtime settings of the properties.
    fn run_action(
        &self,
        ec: &ExecutionContext,
        action: &str,
        parameter_names: Option<serde_json::Value>,
        parameters_map: Option<serde_json::Value>,
    ) -> Result<Value, Error> {
        let runtime_dir = ec.factory().runtime_dir().to_path_buf();
        let launch = Launch {
            python: python_executable(&runtime_dir).map_err(|e| Error::new(e.to_string()))?,
            script: bridge_script(ec)?,
            directory: script_directory(&runtime_dir),
        };
        let request = serde_json::json!({
            "action": action,
            "cti_path": self.cti_path,
            "serial_number": self.serial_number,
            "device_id": self.device_id,
            "parameter_names": parameter_names,
            "parameters_map": parameters_map,
            "runtime_config": runtime_config(),
        });
        let timeout = Duration::from_millis(
            property_number("genicam.process.timeout.ms")
                .filter(|ms| *ms > 0)
                .unwrap_or(120_000),
        );
        tracing::info!("GenICam {action} for camera {}", self.serial_number);
        let answer = self
            .workers
            .exchange(&self.key(), &launch, &request, timeout)
            .map_err(|e| {
                Error::new(format!(
                    "GenICam {action} failed for camera {}: {e}",
                    self.serial_number
                ))
            })?;
        if let Some(error) = answer.get("error") {
            let message = error.as_str().map_or_else(|| error.to_string(), str::to_string);
            return Err(Error::new(format!(
                "GenICam {action} failed for camera {}: {message}",
                self.serial_number
            )));
        }
        bridge_value(&answer)
    }
}

impl Tool for GenicamClient {
    fn methods(&self) -> &'static [&'static str] {
        &[
            "read",
            "write",
            "acquireSingleImage",
            "acquireVideoStream",
            "acquireVideoFile",
            "acquire3dFrame",
            "getFrameBytes",
            "getFramePayload",
            "acquireVisualServoFrame",
            "close",
            "pause",
        ]
    }

    fn call(&self, ec: &ExecutionContext, method: &str, arguments: &[Value]) -> Result<Value, Error> {
        let json = |index: usize| match arguments.get(index) {
            None | Some(Value::Null) => serde_json::Value::Null,
            Some(value) => moqui_lib::json::to_json(value),
        };
        // the names of the bridge's parameters_map, in the order of the method's arguments
        let options = |names: &[&str]| {
            let map: serde_json::Map<String, serde_json::Value> = names
                .iter()
                .enumerate()
                .filter(|(index, _)| !json(*index).is_null())
                .map(|(index, name)| (name.to_string(), json(index)))
                .collect();
            Some(serde_json::Value::Object(map))
        };
        match method {
            "read" => self.run_action(ec, "read", Some(json(0)), None),
            "write" => self.run_action(ec, "write", None, Some(json(0))),
            "acquireSingleImage" => self.run_action(
                ec,
                "single_image",
                None,
                options(&["output_dir", "image_format", "resize_width", "resize_height"]),
            ),
            "acquireVideoStream" => self.run_action(
                ec,
                "video",
                None,
                options(&[
                    "num_frames",
                    "output_dir",
                    "image_format",
                    "resize_width",
                    "resize_height",
                ]),
            ),
            "acquireVideoFile" => self.run_action(
                ec,
                "video_file",
                None,
                options(&[
                    "num_frames",
                    "fps",
                    "output_dir",
                    "video_container",
                    "video_codec",
                    "resize_width",
                    "resize_height",
                ]),
            ),
            "acquire3dFrame" => self.run_action(ec, "acquire_3d_frame", None, None),
            "getFrameBytes" => match self.run_action(ec, "get_frame", None, None)? {
                Value::Map(answer) => Ok(answer.get("jpeg_bytes").cloned().unwrap_or(Value::Null)),
                _ => Ok(Value::Null),
            },
            "getFramePayload" => self.run_action(ec, "get_frame_payload", None, None),
            "acquireVisualServoFrame" => self.run_action(
                ec,
                "visual_servo_frame",
                None,
                options(&[
                    "use_cached",
                    "save_snapshot",
                    "output_dir",
                    "image_format",
                    "resize_width",
                    "resize_height",
                ]),
            ),
            "close" => {
                self.workers.close(&self.key());
                Ok(Value::Null)
            }
            "pause" => {
                let millis = json(0).as_u64().unwrap_or_default().min(60_000);
                std::thread::sleep(Duration::from_millis(millis));
                Ok(Value::Null)
            }
            _ => Err(Error::new(format!("The GenicamClient tool has no method {method}"))),
        }
    }

    fn property(&self, _ec: &ExecutionContext, name: &str) -> Result<Option<Value>, Error> {
        Ok(match name {
            "ctiPath" => Some(Value::string(&self.cti_path)),
            "serialNumber" => Some(Value::string(&self.serial_number)),
            "deviceId" => Some(self.device_id.as_deref().map_or(Value::Null, Value::string)),
            _ => None,
        })
    }
}

/// The file of a script of the component, for the Python interpreter.
pub(crate) fn component_script(ec: &ExecutionContext, location: &str) -> Result<PathBuf, Error> {
    let reference = ec.factory().resource().get(location)?;
    reference
        .file_path()
        .filter(|path| path.is_file())
        .map(PathBuf::from)
        .ok_or_else(|| Error::new(format!("The script {location} is not a file")))
}

fn bridge_script(ec: &ExecutionContext) -> Result<PathBuf, Error> {
    component_script(ec, BRIDGE_SCRIPT)
}

fn property(name: &str) -> Option<String> {
    Some(moqui_lib::env::get_prop_or_env(name).trim().to_string()).filter(|value| !value.is_empty())
}

pub(crate) fn property_number(name: &str) -> Option<u64> {
    property(name).and_then(|value| value.parse().ok())
}

/// `GenicamUtil.buildPythonRuntimeConfig`: the bridge settings that have a property.
fn runtime_config() -> serde_json::Value {
    let mut config = serde_json::Map::new();
    for (key, name) in [
        ("connect_retry_count", "genicam.connection.retry.count"),
        ("connect_retry_backoff_ms", "genicam.connection.retry.backoff.ms"),
        ("fetch_timeout_ms", "genicam.connection.fetch.timeout.ms"),
        ("stream_stop_timeout_ms", "genicam.stream.stop.timeout.ms"),
        ("stream_mock_frame_delay_ms", "genicam.stream.mock.frame.delay.ms"),
        ("servo_max_frame_age_ms", "genicam.servo.max.frame.age.ms"),
    ] {
        if let Some(number) = property_number(name) {
            config.insert(key.into(), number.into());
        }
    }
    config.insert(
        "servo_buffer_source".into(),
        property("genicam.servo.buffer.source")
            .unwrap_or_else(|| "latest".into())
            .into(),
    );
    serde_json::Value::Object(config)
}

/// A bridge answer as a script value: `{"__base64__": "..."}` is a byte buffer, numbers are numbers.
pub(crate) fn bridge_value(json: &serde_json::Value) -> Result<Value, Error> {
    Ok(match json {
        serde_json::Value::Object(map) => {
            if let Some(encoded) = map.get("__base64__").and_then(serde_json::Value::as_str) {
                return base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map(Value::Bytes)
                    .map_err(|e| Error::new(format!("Invalid GenICam buffer: {e}")));
            }
            Value::Map(
                map.iter()
                    .map(|(key, item)| Ok((key.clone(), bridge_value(item)?)))
                    .collect::<Result<_, Error>>()?,
            )
        }
        serde_json::Value::Array(items) => Value::List(items.iter().map(bridge_value).collect::<Result<_, _>>()?),
        other => moqui_lib::json::from_json(other),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_buffers_become_bytes_and_numbers_stay_numbers() {
        let answer = serde_json::json!({"jpeg_bytes": {"__base64__": "AQID"}, "width": 640, "frames": ["a.jpg"],
            "captured_at": 1792152000.25});
        let Value::Map(value) = bridge_value(&answer).unwrap() else {
            panic!("a map")
        };
        assert_eq!(value.get("jpeg_bytes"), Some(&Value::Bytes(vec![1, 2, 3])));
        assert_eq!(value.get("width"), Some(&Value::I64(640)));
        assert_eq!(value.get("frames"), Some(&Value::List(vec![Value::string("a.jpg")])));
        assert!(matches!(value.get("captured_at"), Some(Value::Decimal(_))));
    }
}
