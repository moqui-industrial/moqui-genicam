# Port from the Java/Groovy Version

The `mr-moqui` branch restarts from `master` (`d790fc2`), the Java/Groovy component, and replaces Groovy and JEP. It
follows the layout of moqui-device and moqui-device-gateway on MR-Moqui:

- **Services.** A service written as XML Actions stays XML Actions, with JavaScript expressions. The services keep
  their names, parameters and data; the XML Actions that find the `Device` and the `DeviceConnection` are the ones
  of the Groovy version.
- **Groovy scripts.** The logic of a Groovy script is in the JavaScript library
  `service/moqui/genicam/Genicam.js`, called with
  `ec.resource.script(...).execute(operation, context)`, as moqui-device calls `Devices.js`.
- **Groovy classes.** A Groovy class is a Rust tool, taken with `ec.getTool(...)`.
- **Python.** The Python scripts are the camera side and are not ported: Harvesters and the GenTL producer of the
  camera vendor (Teledyne FLIR Spinnaker) are what the camera SDK offers. `genicam_bridge.py` only gains, at its end,
  the loop that runs it as a worker process in place of JEP. `multi_camera_capture.py` is unchanged.

## Migration map

| Java/Groovy | MR-Moqui |
| --- | --- |
| `org.moqui.genicam.GenicamClient` | Rust tool `GenicamClient` (`src/client.rs`): the same methods, plus `pause(millis)`, the `Thread.sleep` of the live stream |
| `org.moqui.genicam.GenicamMultiCameraRunner` | Rust tool `GenicamMultiCameraRunner` (`src/multi_camera.rs`): the same process, request on stdin, timeout and report |
| `org.moqui.genicam.GenicamUtil` | `Genicam.js` (configuration, serial number, parameters map, Tensor records); `buildPythonRuntimeConfig`, `resolvePythonExecutable` and `resolveComponentScriptPath` in the Rust tools |
| JEP interpreter (`JepToolFactory`, `moqui-jep`) | A Python worker process per camera (`src/worker.rs`), JSON lines on stdin and stdout, kept between calls as the interpreter was |
| Groovy scripts of `GenicamServices.xml` | `Genicam.js` operations: `runRequest`, `singleImage`, `videoStream`, `videoFile`, `multiVideoFile`, `visualServoFrame`, `liveMjpeg`, `acquire3dFrame`, `cleanTensors` |
| `GenicamUtil.syncParameters` (Parameter update and `ParameterLog` row per value) | `moqui.device.DeviceAcquisitionServices.store#Samples`, as the drivers of moqui-device-gateway |
| Bridge writes with `ec` in JEP: `DeviceContent` rows, `Device` status `DbsErrorStop` | `Genicam.js` (`storeDeviceContent`, `markDeviceError`); the worker has no ExecutionContext |
| `build.gradle`, Spock tests (`src/test/groovy`) | `Cargo.toml`; `test/Genicam.spec.js`, with the real-camera test behind `genicam_real_enabled` |

## Differences from the Java version

- **Stored values.** A read or write stores the camera's values through `store#Samples`:
  - the Parameter is updated only when the value changed;
  - `ParameterLog` rows are written only for a `DrpLogging` request.

  The Groovy version wrote a `ParameterLog` row for every value, numbered with `count + 1`.
- **Results of `run#GenICamRequest`.** It returns `status`, `samples` and `storedCount`, which
  `run#DeviceRequest` passes on.
- **Connections.** The connection of an acquisition must belong to the requested camera, and the serial number comes
  from `PhysicalDevice.serialNumber`, where moqui-device keeps it.
- **3D frames and retention.**
  - `acquire#GenICam3DFrame` sets `TensorContent.contentDate`. The Groovy version left it empty, so
    `clean#GenICamTensors` never found a frame.
  - `clean#GenICamTensors` deletes only GenICam tensors under the tensor directory (`outputDir` or
    `genicam.tensors.path`); the Groovy version deleted every NPY tensor.
- **Live stream.** `stream#LiveMjpeg` needs an authenticated user with permission on the service. It writes each
  frame with `ec.web.response.write` and sends it with `flushBuffer()`.
  - A response goes out while it is written only from a servlet script, so the component ships
    `script/GenicamStreamServlet.js`, which the application declares in its conf.
  - A script cannot see a client that went away, so the stream ends after `maxFrames` frames, as the Groovy loop did
    without an `IOException`.
- **Worker timeout.** `genicam.process.timeout.ms` limits the answer of a camera's worker. A call that times out
  stops the worker and its command is not sent again.
- **Python interpreter.** It is `runtime/python_venv` as in the Java version, then `PYTHON`, then the `PATH`.
- **Data.**
  - The enumerations `DtVisionCamera`, `DcdGenicam` and `DritString` are seed data (`GenicamData.xml`).
  - `GenicamTestData.xml` is `type="test"`. Its cameras use `GenicamCameraStatusFlow` and keep their serial number
    in `PhysicalDevice`.
