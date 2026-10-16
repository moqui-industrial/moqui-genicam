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
