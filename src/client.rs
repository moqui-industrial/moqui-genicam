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

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use moqui_core::context::{Error as CoreError, ExecutionContext};
use moqui_core::entity::EntityValue;
use moqui_core::native::{Tool, ToolFactory};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use crate::util;

pub struct GenicamClient {
    pub cti_path: String,
    pub serial_number: String,
    pub device_id: Option<String>,
}

impl GenicamClient {
    pub fn new(cti_path: String, serial_number: String, device_id: Option<String>) -> Self {
        Self {
            cti_path,
            serial_number,
            device_id,
        }
    }

    pub fn run_action(
        &self,
        ec: &ExecutionContext,
        action: &str,
        param_names: Option<&[String]>,
        params_map: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, LibError> {
        let python_exe = util::resolve_python_executable(ec)?;
        let script_path = util::resolve_component_script_path(ec, "genicam_bridge.py")?;
        let runtime_config = util::build_python_runtime_config();

        let mut payload = serde_json::Map::new();
        payload.insert("command".to_string(), serde_json::Value::from("run-action"));
        payload.insert("action".to_string(), serde_json::Value::from(action));
        payload.insert("cti_path".to_string(), serde_json::Value::from(self.cti_path.clone()));
        payload.insert("serial_number".to_string(), serde_json::Value::from(self.serial_number.clone()));
        if let Some(did) = &self.device_id {
            payload.insert("device_id".to_string(), serde_json::Value::from(did.clone()));
        }
        if let Some(names) = param_names {
            payload.insert("parameter_names".to_string(), serde_json::to_value(names).unwrap_or(serde_json::Value::Null));
        }
        if let Some(map) = params_map {
            payload.insert("parameters_map".to_string(), map.clone());
        }
        payload.insert("runtime_config".to_string(), runtime_config);

        let factory = ec.factory().get_tool_factory_typed::<GenicamToolFactory>("GenicamClient")
            .ok_or_else(|| LibError::new("GenicamClient tool factory is not configured"))?;
        let timeout_ms = util::resolve_config_property("genicam.process.timeout.ms", None)
            .and_then(|value| value.parse::<u64>().ok()).filter(|value| *value > 0).unwrap_or(120_000);
        let parsed = factory.workers.exchange((self.cti_path.clone(), self.serial_number.clone()),
            &python_exe, &script_path, &serde_json::Value::Object(payload), std::time::Duration::from_millis(timeout_ms))
            .inspect_err(|_| self.mark_device_error(ec))?;

        if let Some(err_val) = parsed.get("error") {
            self.mark_device_error(ec);
            return Err(LibError::new(format!("GenICam camera error: {err_val}")));
        }

        Ok(parsed)
    }

    pub fn mark_device_error(&self, ec: &ExecutionContext) {
        if let Some(did) = &self.device_id
            && let Ok(entity) = util::get_entity(ec)
                && let Ok(Some(dev)) = entity.find("moqui.device.Device").and_then(|f| f.eq("deviceId", Value::string(did)).one()) {
                    let parameters = std::collections::BTreeMap::from([
                        ("deviceId".into(), Value::string(did)),
                        ("statusId".into(), Value::string("DbsErrorStop")),
                    ]);
                    let _ = dev;
                    if let Err(error) = moqui_core::service::sync("update#moqui.device.Device", &parameters) {
                        tracing::warn!("Could not record GenICam device error status: {error}");
                    }
                }
    }

    pub fn read(&self, ec: &ExecutionContext, request: &EntityValue, request_items: &[Value]) -> Result<(), LibError> {
        let req_type = request.get("requestTypeEnumId").and_then(|v| v.as_str()).unwrap_or_default();
        if req_type != "DrtRead" {
            let req_name = request.get("requestName").and_then(|v| v.as_str()).unwrap_or_default();
            return Err(LibError::new(format!(
                "The device request with name '{req_name}' is not a read request (found '{req_type}')."
            )));
        }

        let mut param_names = Vec::new();
        for item in request_items {
            let q = match item {
                Value::Map(m) => m.get("query").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                Value::Object(obj) => {
                    if let Some(ev) = obj.downcast::<EntityValue>() {
                        ev.get("query").and_then(|v| v.as_str()).unwrap_or_default().to_string()
                    } else {
                        String::new()
                    }
                }
                _ => String::new(),
            };
            if !q.is_empty() {
                param_names.push(q);
            }
        }

        tracing::info!("Executing GenICam Read request for camera {}", self.serial_number);
        let results = self.run_action(ec, "read", Some(&param_names), None)?;
        if let Some(map) = results.as_object() {
            util::sync_parameters(ec, request_items, map)?;
        }
        Ok(())
    }

    pub fn write(&self, ec: &ExecutionContext, request: &EntityValue, request_items: &[Value]) -> Result<(), LibError> {
        let req_type = request.get("requestTypeEnumId").and_then(|v| v.as_str()).unwrap_or_default();
        if req_type != "DrtWrite" {
            let req_name = request.get("requestName").and_then(|v| v.as_str()).unwrap_or_default();
            return Err(LibError::new(format!(
                "The device request with name '{req_name}' is not a write request (found '{req_type}')."
            )));
        }

        let params_map = util::build_parameters_map(ec, request_items)?;
        let json_map = serde_json::Value::Object(params_map);
        tracing::info!("Executing GenICam Write request for camera {} with parameters: {:?}", self.serial_number, json_map);

        let results = self.run_action(ec, "write", None, Some(&json_map))?;
        if let Some(map) = results.as_object() {
            util::sync_parameters(ec, request_items, map)?;
        }
        Ok(())
    }

    pub fn acquire_single_image(
        &self,
        ec: &ExecutionContext,
        output_dir: Option<&str>,
        image_format: Option<&str>,
        resize_width: Option<i64>,
        resize_height: Option<i64>,
    ) -> Result<serde_json::Value, LibError> {
        let mut map = serde_json::Map::new();
        if let Some(od) = output_dir {
            map.insert("output_dir".to_string(), serde_json::Value::from(od));
        }
        if let Some(fmt) = image_format {
            map.insert("image_format".to_string(), serde_json::Value::from(fmt));
        }
        if let Some(w) = resize_width {
            map.insert("resize_width".to_string(), serde_json::Value::from(w));
        }
        if let Some(h) = resize_height {
            map.insert("resize_height".to_string(), serde_json::Value::from(h));
        }

        self.run_action(ec, "single_image", None, Some(&serde_json::Value::Object(map)))
    }

    pub fn acquire_video_stream(
        &self,
        ec: &ExecutionContext,
        num_frames: i64,
        output_dir: Option<&str>,
        image_format: Option<&str>,
        resize_width: Option<i64>,
        resize_height: Option<i64>,
    ) -> Result<serde_json::Value, LibError> {
        let mut map = serde_json::Map::new();
        map.insert("num_frames".to_string(), serde_json::Value::from(num_frames));
        if let Some(od) = output_dir {
            map.insert("output_dir".to_string(), serde_json::Value::from(od));
        }
        if let Some(fmt) = image_format {
            map.insert("image_format".to_string(), serde_json::Value::from(fmt));
        }
        if let Some(w) = resize_width {
            map.insert("resize_width".to_string(), serde_json::Value::from(w));
        }
        if let Some(h) = resize_height {
            map.insert("resize_height".to_string(), serde_json::Value::from(h));
        }

        self.run_action(ec, "video", None, Some(&serde_json::Value::Object(map)))
    }

    #[allow(clippy::too_many_arguments)] // Existing JavaScript tool signature.
    pub fn acquire_video_file(
        &self,
        ec: &ExecutionContext,
        num_frames: i64,
        fps: f64,
        output_dir: Option<&str>,
        video_container: Option<&str>,
        video_codec: Option<&str>,
        resize_width: Option<i64>,
        resize_height: Option<i64>,
    ) -> Result<serde_json::Value, LibError> {
        let mut map = serde_json::Map::new();
        map.insert("num_frames".to_string(), serde_json::Value::from(num_frames));
        map.insert("fps".to_string(), serde_json::Value::from(fps));
        if let Some(od) = output_dir {
            map.insert("output_dir".to_string(), serde_json::Value::from(od));
        }
        if let Some(vc) = video_container {
            map.insert("video_container".to_string(), serde_json::Value::from(vc));
        }
        if let Some(cd) = video_codec {
            map.insert("video_codec".to_string(), serde_json::Value::from(cd));
        }
        if let Some(w) = resize_width {
            map.insert("resize_width".to_string(), serde_json::Value::from(w));
        }
        if let Some(h) = resize_height {
            map.insert("resize_height".to_string(), serde_json::Value::from(h));
        }

        self.run_action(ec, "video_file", None, Some(&serde_json::Value::Object(map)))
    }

    pub fn acquire_3d_frame(&self, ec: &ExecutionContext) -> Result<serde_json::Value, LibError> {
        self.run_action(ec, "acquire_3d_frame", None, None)
    }

    pub fn get_frame_bytes(&self, ec: &ExecutionContext) -> Result<Vec<u8>, LibError> {
        let res = self.run_action(ec, "get_frame", None, None)?;
        if let Some(base64_str) = res.get("__base64__").and_then(|v| v.as_str()) {
            return BASE64.decode(base64_str).map_err(|e| LibError::new(format!("Base64 decode error: {e}")));
        }
        if let Some(jpeg_val) = res.get("jpeg_bytes") {
            if jpeg_val.get("__base64__").is_some() { return decode_bridge_bytes(jpeg_val); }
            if let Some(b64) = jpeg_val.as_str() {
                return BASE64.decode(b64).map_err(|e| LibError::new(format!("Base64 decode error: {e}")));
            }
            if let Some(arr) = jpeg_val.as_array() {
                return Ok(arr.iter().filter_map(|b| b.as_u64().map(|n| n as u8)).collect());
            }
        }
        Ok(Vec::new())
    }

    pub fn get_frame_payload(&self, ec: &ExecutionContext) -> Result<serde_json::Value, LibError> {
        self.run_action(ec, "get_frame_payload", None, None)
    }

    #[allow(clippy::too_many_arguments)] // Existing JavaScript tool signature.
    pub fn acquire_visual_servo_frame(
        &self,
        ec: &ExecutionContext,
        use_cached: bool,
        save_snapshot: bool,
        output_dir: Option<&str>,
        image_format: Option<&str>,
        resize_width: Option<i64>,
        resize_height: Option<i64>,
    ) -> Result<serde_json::Value, LibError> {
        let mut map = serde_json::Map::new();
        map.insert("use_cached".to_string(), serde_json::Value::from(use_cached));
        map.insert("save_snapshot".to_string(), serde_json::Value::from(save_snapshot));
        if let Some(od) = output_dir {
            map.insert("output_dir".to_string(), serde_json::Value::from(od));
        }
        if let Some(fmt) = image_format {
            map.insert("image_format".to_string(), serde_json::Value::from(fmt));
        }
        if let Some(w) = resize_width {
            map.insert("resize_width".to_string(), serde_json::Value::from(w));
        }
        if let Some(h) = resize_height {
            map.insert("resize_height".to_string(), serde_json::Value::from(h));
        }

        self.run_action(ec, "visual_servo_frame", None, Some(&serde_json::Value::Object(map)))
    }

    pub fn close(&self, ec: &ExecutionContext) -> Result<(), LibError> {
        if let Some(factory) = ec.factory().get_tool_factory_typed::<GenicamToolFactory>("GenicamClient") {
            factory.workers.close(&(self.cti_path.clone(), self.serial_number.clone()));
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct GenicamToolFactory { pub(crate) workers: Arc<crate::worker::WorkerPool> }

impl ToolFactory for GenicamToolFactory {
    fn name(&self) -> &str {
        "GenicamClient"
    }

    fn destroy(&self) { self.workers.close_all(); }

    fn get_instance(&self, parameters: &[Value]) -> Result<Arc<dyn Tool>, CoreError> {
        let cti_path = parameters.first()
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let serial_number = parameters.get(1)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let device_id = parameters.get(2)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());

        let client = GenicamClient::new(cti_path, serial_number, device_id);
        Ok(Arc::new(GenicamTool {
            client: Arc::new(client),
        }))
    }

    fn type_declaration(&self) -> Option<&'static str> {
        Some(
            "{ read(request: any, requestItemList: any[]): void; \
               write(request: any, requestItemList: any[]): void; \
               acquireSingleImage(outputDir?: string, imageFormat?: string, resizeWidth?: number, resizeHeight?: number): any; \
               acquireVideoStream(numFrames?: number, outputDir?: string, imageFormat?: string, resizeWidth?: number, resizeHeight?: number): any; \
               acquireVideoFile(numFrames?: number, fps?: number, outputDir?: string, videoContainer?: string, videoCodec?: string, resizeWidth?: number, resizeHeight?: number): any; \
               acquire3dFrame(): any; \
               getFrameBytes(): number[]; \
               getFramePayload(): any; \
               acquireVisualServoFrame(useCached?: boolean, saveSnapshot?: boolean, outputDir?: string, imageFormat?: string, resizeWidth?: number, resizeHeight?: number): any; \
               close(): void; \
               readonly ctiPath: string; \
               readonly serialNumber: string; \
               readonly deviceId: string | null }"
        )
    }
}

pub struct GenicamTool {
    pub client: Arc<GenicamClient>,
}

impl Tool for GenicamTool {
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
        ]
    }

    fn call(&self, ec: &ExecutionContext, method: &str, args: &[Value]) -> Result<Value, CoreError> {
        match method {
            "read" => {
                let request = args.first().and_then(|v| match v {
                    Value::Object(obj) => obj.downcast::<EntityValue>(),
                    _ => None,
                }).ok_or_else(|| CoreError::new("read requires request EntityValue as first argument"))?;

                let request_items = match args.get(1) {
                    Some(Value::List(list)) => list.clone(),
                    _ => Vec::new(),
                };

                self.client.read(ec, &request, &request_items).map_err(|e| CoreError::new(e.to_string()))?;
                Ok(Value::Null)
            }
            "write" => {
                let request = args.first().and_then(|v| match v {
                    Value::Object(obj) => obj.downcast::<EntityValue>(),
                    _ => None,
                }).ok_or_else(|| CoreError::new("write requires request EntityValue as first argument"))?;

                let request_items = match args.get(1) {
                    Some(Value::List(list)) => list.clone(),
                    _ => Vec::new(),
                };

                self.client.write(ec, &request, &request_items).map_err(|e| CoreError::new(e.to_string()))?;
                Ok(Value::Null)
            }
            "acquireSingleImage" => {
                let output_dir = args.first().and_then(|v| v.as_str());
                let image_format = args.get(1).and_then(|v| v.as_str());
                let resize_width = args.get(2).and_then(|v| match v {
                    Value::I64(n) => Some(*n),
                    Value::I32(n) => Some(*n as i64),
                    _ => None,
                });
                let resize_height = args.get(3).and_then(|v| match v {
                    Value::I64(n) => Some(*n),
                    Value::I32(n) => Some(*n as i64),
                    _ => None,
                });

                let res = self.client.acquire_single_image(ec, output_dir, image_format, resize_width, resize_height)
                    .map_err(|e| CoreError::new(e.to_string()))?;
                json_to_moqui_value(&res)
            }
            "acquireVideoStream" => {
                let num_frames = args.first().and_then(|v| match v {
                    Value::I64(n) => Some(*n),
                    Value::I32(n) => Some(*n as i64),
                    _ => None,
                }).unwrap_or(10);
                let output_dir = args.get(1).and_then(|v| v.as_str());
                let image_format = args.get(2).and_then(|v| v.as_str());
                let resize_width = args.get(3).and_then(|v| match v {
                    Value::I64(n) => Some(*n),
                    Value::I32(n) => Some(*n as i64),
                    _ => None,
                });
                let resize_height = args.get(4).and_then(|v| match v {
                    Value::I64(n) => Some(*n),
                    Value::I32(n) => Some(*n as i64),
                    _ => None,
                });

                let res = self.client.acquire_video_stream(ec, num_frames, output_dir, image_format, resize_width, resize_height)
                    .map_err(|e| CoreError::new(e.to_string()))?;
                json_to_moqui_value(&res)
            }
            "acquireVideoFile" => {
                let num_frames = args.first().and_then(|v| match v {
                    Value::I64(n) => Some(*n),
                    Value::I32(n) => Some(*n as i64),
                    _ => None,
                }).unwrap_or(60);
                let fps = args.get(1).and_then(|v| match v {
                    Value::F64(n) => Some(*n),
                    Value::I64(n) => Some(*n as f64),
                    Value::I32(n) => Some(*n as f64),
                    _ => None,
                }).unwrap_or(15.0);
                let output_dir = args.get(2).and_then(|v| v.as_str());
                let video_container = args.get(3).and_then(|v| v.as_str());
                let video_codec = args.get(4).and_then(|v| v.as_str());
                let resize_width = args.get(5).and_then(|v| match v {
                    Value::I64(n) => Some(*n),
                    Value::I32(n) => Some(*n as i64),
                    _ => None,
                });
                let resize_height = args.get(6).and_then(|v| match v {
                    Value::I64(n) => Some(*n),
                    Value::I32(n) => Some(*n as i64),
                    _ => None,
                });

                let res = self.client.acquire_video_file(ec, num_frames, fps, output_dir, video_container, video_codec, resize_width, resize_height)
                    .map_err(|e| CoreError::new(e.to_string()))?;
                json_to_moqui_value(&res)
            }
            "acquire3dFrame" => {
                let res = self.client.acquire_3d_frame(ec).map_err(|e| CoreError::new(e.to_string()))?;
                json_to_moqui_value(&res)
            }
            "getFrameBytes" => {
                let bytes = self.client.get_frame_bytes(ec).map_err(|e| CoreError::new(e.to_string()))?;
                Ok(Value::Bytes(bytes))
            }
            "getFramePayload" => {
                let res = self.client.get_frame_payload(ec).map_err(|e| CoreError::new(e.to_string()))?;
                json_to_moqui_value(&res)
            }
            "acquireVisualServoFrame" => {
                let use_cached = args.first().and_then(|v| match v {
                    Value::Bool(b) => Some(*b),
                    _ => None,
                }).unwrap_or(true);
                let save_snapshot = args.get(1).and_then(|v| match v {
                    Value::Bool(b) => Some(*b),
                    _ => None,
                }).unwrap_or(false);
                let output_dir = args.get(2).and_then(|v| v.as_str());
                let image_format = args.get(3).and_then(|v| v.as_str());
                let resize_width = args.get(4).and_then(|v| match v {
                    Value::I64(n) => Some(*n),
                    Value::I32(n) => Some(*n as i64),
                    _ => None,
                });
                let resize_height = args.get(5).and_then(|v| match v {
                    Value::I64(n) => Some(*n),
                    Value::I32(n) => Some(*n as i64),
                    _ => None,
                });

                let res = self.client.acquire_visual_servo_frame(ec, use_cached, save_snapshot, output_dir, image_format, resize_width, resize_height)
                    .map_err(|e| CoreError::new(e.to_string()))?;
                json_to_moqui_value(&res)
            }
            "close" => {
                self.client.close(ec).map_err(|e| CoreError::new(e.to_string()))?;
                Ok(Value::Null)
            }
            _ => Err(CoreError::new(format!("GenicamTool has no method '{method}'"))),
        }
    }

    fn property(&self, _ec: &ExecutionContext, name: &str) -> Result<Option<Value>, CoreError> {
        match name {
            "ctiPath" => Ok(Some(Value::string(&self.client.cti_path))),
            "serialNumber" => Ok(Some(Value::string(&self.client.serial_number))),
            "deviceId" => Ok(Some(self.client.device_id.as_deref().map(Value::string).unwrap_or(Value::Null))),
            _ => Ok(None),
        }
    }
}

pub fn json_to_moqui_value(val: &serde_json::Value) -> Result<Value, CoreError> {
    if val.get("__base64__").is_some() {
        return decode_bridge_bytes(val).map(Value::Bytes).map_err(|error| CoreError::new(error.to_string()));
    }
    match val {
        serde_json::Value::Null => Ok(Value::Null),
        serde_json::Value::Bool(b) => Ok(Value::Bool(*b)),
        serde_json::Value::Number(num) => {
            if let Some(i) = num.as_i64() {
                Ok(Value::I64(i))
            } else if let Some(f) = num.as_f64() {
                Ok(Value::F64(f))
            } else {
                Ok(Value::string(num.to_string()))
            }
        }
        serde_json::Value::String(s) => Ok(Value::string(s)),
        serde_json::Value::Array(arr) => {
            let mut list = Vec::new();
            for item in arr {
                list.push(json_to_moqui_value(item)?);
            }
            Ok(Value::List(list))
        }
        serde_json::Value::Object(obj) => {
            let mut map = indexmap::IndexMap::new();
            for (k, v) in obj {
                map.insert(k.clone(), json_to_moqui_value(v)?);
            }
            Ok(Value::Map(map))
        }
    }
}

pub fn decode_bridge_bytes(value: &serde_json::Value) -> Result<Vec<u8>, LibError> {
    let encoded = value.as_str().or_else(|| value.get("__base64__").and_then(|v| v.as_str()))
        .ok_or_else(|| LibError::new("Expected a base64 bridge buffer"))?;
    BASE64.decode(encoded).map_err(|error| LibError::new(format!("Invalid bridge base64 buffer: {error}")))
}
