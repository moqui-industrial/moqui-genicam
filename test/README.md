# GenICam regression tests

Use MR-Moqui master with the mr-moqui branches of moqui-math, moqui-device and
moqui-genicam in runtime/component. These tests need a Python interpreter with
NumPy and OpenCV, but no camera SDK, real camera, Mantle, MQTT or ONNX Runtime.
Install the component requirements in runtime/python_venv, or select an absolute
interpreter with PYTHON. GenicamTestData.xml provides the explicit mock camera.

From the framework root:

```bash
./moq build --debug
./moq test --debug moqui-genicam
```

The default runner uses a scratch database. For a disposable already loaded
runtime, load the component seeds then use `--db existing`. Never target a
production database: the retention feature creates and removes test artifacts.

The tests cover binary frames, timestamps, image content references, 3D NPY
persistence, connection ownership and isolation of tensor retention. Rust unit
tests also check worker reuse and termination of a stalled worker without replay.
The 13-input obstacle network is demonstrated/tested in moqui-device; GenICam
itself is an acquisition component and does not recognize obstacles.

Python-only bridge boundary tests (from the component directory):

```bash
python3 -m unittest discover -s test -p 'test_*.py'
```

See [VALIDATION.md](VALIDATION.md) for executed checks and their limits.

For HTTP streaming, mount the authenticated `GenicamStream.xml` screen as
explained in the component README. Load the mock-camera test data into an
isolated application runtime and start its HTTP server. From the component:

```bash
python3 test/mjpeg_http.py --url http://127.0.0.1:8080/GenicamStream/stream \
  --cookie 'moqui.session=YOUR_AUTHENTICATED_SESSION_COOKIE'
```

Use the actual cookie name/value returned by your application. The test requests
the explicit `FLIR_CAMERA_1` mock fixture, verifies progressive JPEG parts and
finite closure, disconnects an unlimited stream, then captures again. It needs
the MR-Moqui native response-stream extension. It creates no image snapshots.
