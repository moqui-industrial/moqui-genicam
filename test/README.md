# GenICam regression tests

`Genicam.spec.js` holds the tests of the Java version (`GenicamServiceTests.groovy`) on MR-Moqui. They use the mock
camera `FLIR_CAMERA_1` of `data/GenicamTestData.xml`, which the Python bridge simulates, so they need:

- MR-Moqui with moqui-device, moqui-math and this component on their `mr-moqui` branches in `runtime/components/addon`;
- a Python with NumPy and OpenCV: `runtime/python_venv`, `PYTHON`, or `python3` on the `PATH`.

They do not need a camera, a camera SDK, Harvesters or Mantle.

From the framework directory:

```sh
./moq build --debug
./moq test --debug moqui-genicam
```

The runner uses a scratch database. Never run the tests against a production database: they create and delete
DeviceContent, Tensor and Parameter rows and their files.

## What the tests cover

- reads and writes through `run#DeviceRequest` and `store#Samples`;
- the streaming thread and its latest frame, as a file and as the JPEG of the live view;
- single images, frame series, video files and multi-camera recordings;
- visual servo frames;
- 3D frames as Tensors, and their retention;
- refusal of the connection of another device.

## Real camera

The feature "a real FLIR camera ..." runs only with `genicam_real_enabled=true`. It uses `FLIR_CAMERA_REAL_19176722`
and its connection `FlirCameraConnectionReal`. Set `genicam_real_cti_path` to the Spinnaker GenTL producer (`.cti`)
of the machine.

## Rust unit tests

From the component directory, inside the framework:

```sh
cargo test -p moqui-genicam
```

They check:

- the conversion of the bridge's buffers;
- a stalled worker, which is stopped without its request being sent again;
- the worker of the mock camera, closed and reopened, and refused after shutdown.
